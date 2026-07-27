---
name: actor
description: >
  Become an actor in a multi-agent work network. Opens a mailbox (FIFO) at
  `<workspace_root>/ai/agents/<actor-name>` and processes messages in a
  listen→act→listen loop until instructed to terminate. Coworkers are
  discovered under `ai/agents/` relative to the same workspace root.
---

# Actor Skill

When you are told to use this skill, you become an **actor** in a
multi-agent work network. Actors communicate through FIFO mailboxes
(via `synctell`). Each actor has a personal mailbox; all mailboxes
live under `ai/agents/` in a shared **workspace root** directory.

## Philosophy

Actors are **autonomous long-lived agents**. Each actor:
- Manages its own listen→act→listen loop
- Discovers coworkers dynamically
- Can message peers directly (peer-to-peer)
- Reacts to incoming work, does it, then goes back to listening
- Exits cleanly only on a shutdown signal

The orchestrator's role is to **start the network, monitor for wayward
actors, and coordinate shutdown** — not to micromanage each step.

## Actor Identity

You are told two things:

- **actor name** — your identity (e.g., `"writer"`, `"editor"`, `"worker-a"`).
- **workspace root** — the shared base directory where the `ai/agents/`
  network lives. Defaults to the **current working directory** of the
  agent process.

Your mailbox: `<workspace_root>/ai/agents/<actor-name>/inbox.fifo`
Peer mailboxes: discovered by scanning `<workspace_root>/ai/agents/` for
directories containing `inbox.fifo`.

## Protocol Overview

```
<workspace_root>/
  └── ai/agents/
        ├── editor/       ← mailbox: editor/inbox.fifo
        ├── writer/       ← mailbox: writer/inbox.fifo
        ├── proofreader/  ← mailbox: proofreader/inbox.fifo
        └── publisher/    ← mailbox: publisher/inbox.fifo
```

The FIFO's existence signals that the actor is listening.
The directory name is the actor name.

## Activation Protocol

When you are told to "become an actor" or "use the actor skill":

### Step 1 — Create the mailbox directory

```bash
mkdir -p "<workspace_root>/ai/agents/<actor-name>"
```

### Step 2 — Start a linger reader on your mailbox FIFO

**MCP (recommended):**
```
synctell_read_start_linger(path="<workspace_root>/ai/agents/<actor-name>/inbox.fifo", timeout=0)
```

**CLI alternative:**
```bash
synctell read "<workspace_root>/ai/agents/<actor-name>/inbox.fifo" &
```

The linger reader creates the FIFO and stays alive for multiple writers.
It blocks until messages arrive.

### Step 3 — Announce your presence

Log a message like:
```
ACTOR <actor-name> READY: listening at <path>
```

### Step 3b — Wait for Coworkers (Coordinator Actors Only)

If your role requires coordinating with specific coworkers (e.g., an
editor who assigns work to writers), you must wait for their mailboxes
to appear before proceeding. Do NOT assume they exist yet — actors start
up asynchronously and may not have created their FIFOs yet.

**Polling with timeout:**

```
start = now()
while now() - start < 200 seconds:
    coworkers = list directories under <workspace_root>/ai/agents/
    if all expected coworkers have inbox.fifo:
        break
    sleep(2 seconds)  # wait before retrying
if not all coworkers found:
    print "ERROR: Coworkers X, Y, Z never appeared"
    exit (or continue with partial workforce)
```

**MCP implementation:** Use `filesystem` tools to list `<workspace_root>/ai/agents/`
and check for `inbox.fifo` in each subdirectory. If a directory exists
but has no `inbox.fifo` yet, the actor is still starting up.

**CLI implementation:**
```bash
for i in $(seq 1 100); do
    all_found=true
    for actor in writer proofreader publisher; do
        if [ ! -f "<workspace_root>/ai/agents/$actor/inbox.fifo" ]; then
            all_found=false
            break
        fi
    done
    $all_found && break
    sleep 2
done
```

This pattern prevents the **startup race** where one actor tries to
write to a peer's mailbox before the peer has created its FIFO.

### Step 4 — Listen → Act → Listen loop

This is the core pattern. Each actor runs this loop autonomously:

```
loop:
  message ← synctell_read_still_linger(path="<inbox>", timeout=0)
  if message contains "shutdown":
    break
  else:
    perform work based on message
    # optionally write results to peers' inboxes
    # loop continues
```

**Important:** Use `synctell_read_still_linger` (NOT `read_start_linger`)
for each read in the loop. The linger reader was started once in Step 2
and stays alive — `read_still_linger` just pulls the next message from it.

### Step 5 — Termination

When you receive a `{"command": "shutdown"}` message:

1. **Stop the linger reader:**
   - **MCP:** `synctell_read_stop_linger(path="<inbox>")`
   - **CLI:** Send SIGTERM to the `synctell read` process.

2. **Remove the directory:**
   ```bash
   rmdir "<workspace_root>/ai/agents/<actor-name>"
   ```
   (Use `rm -rf` if directory still has files.)

3. **Exit cleanly.**

## Messaging Coworkers

**MCP:** `synctell_write(path="<workspace_root>/ai/agents/<coworker>/inbox.fifo", message=...)`

**CLI:**
```bash
synctell write "<workspace_root>/ai/agents/<coworker-name>/inbox.fifo" "message"
```

The write blocks until the recipient's FIFO exists and is read.
This provides natural backpressure — the writer waits until the
receiver is ready. If the FIFO doesn't exist, the coworker is not
listening and the message cannot be delivered.

## Discovering Coworkers

**MCP:** Use the filesystem tool to list directories under
`<workspace_root>/ai/agents/` and check for `inbox.fifo`.

**CLI:**
```bash
ls -d "<workspace_root>/ai/agents/"*/inbox.fifo 2>/dev/null
```

Each entry is a live mailbox. The directory name is the actor name.

**Important:** When writing to a coworker, `synctell_write` blocks until
the FIFO exists and is read. This means if the coworker hasn't started
yet, your write will hang indefinitely. **Always check that the
target FIFO exists before writing.** Poll for it with a timeout if
needed.

## Using the Skill as a Subagent (Delegate)

When you are started as a **delegate** with the actor skill, you should:

1. You are given `actor_name` and `workspace_root` as parameters.
2. Follow the Activation Protocol (steps 1-5 above).
3. Use generous `max_turns` (~100+) when the orchestrator launches you
   so you have room to run the full listen→act→listen loop.
4. In the listen loop, read messages and perform work. For each message
   that requires complex work, you may delegate sub-subagents if needed,
   or just do the work directly.
5. On shutdown, clean up and exit.

## Orchestration Patterns (for the Orchestrator)

The orchestrator delegates actors, then uses `synctell` to coordinate:

### Broadcast (fan-out)

Send the same message to all actors simultaneously.

**MCP:**
```
1. synctell_broadcast_start(path, outputs=[...all worker inboxes])
2. synctell_write(path, message)  — one or more writes
3. synctell_broadcast_stop(path)
```

**CLI:**
```bash
synctell broadcast instructions.fifo $(ls -d "agents/"*/inbox.fifo)
synctell write instructions.fifo '{"task": "process dataset"}'
```

### Round-robin (load balancing)

Distribute work items evenly across workers.

**MCP:**
```
1. synctell_roundrobin_start(path, outputs=[...worker inboxes])
2. synctell_write(path, task-1)
   synctell_write(path, task-2)
   synctell_write(path, task-3)
3. synctell_roundrobin_stop(path)
```

**CLI:**
```bash
synctell roundrobin tasks.fifo worker-a/inbox.fifo worker-b/inbox.fifo
synctell write tasks.fifo '{"task": "item-1"}'
synctell write tasks.fifo '{"task": "item-2"}'
```

### Peer-to-peer

Actors write directly to each other's mailboxes.

## Wayward Agent Detection & Recovery

Subagents can sometimes get stuck — they hit their action limit, loop
on a bug, or hang waiting for a message that will never come. The
orchestrator should handle this:

### Detection

- **Timeout:** If you haven't heard from a subagent after a reasonable
  time (considering the task), it may be stuck.
- **Stale mailbox:** If a subagent's inbox.fifo exists but the subagent
  hasn't responded to a ping for too long, investigate.
- **Action limit:** Delegates return with "reached max consecutive
  actions" output. This is detectable when you `load(taskId)`.

### Recovery

1. **Send SIGTERM** to the wayward subagent's process (if you have its
   PID). This gives it a chance to clean up.
2. **Send SIGKILL** if SIGTERM doesn't work within a few seconds.
3. **Clean up the actor's mailbox:**
   - Remove the FIFO: `rm -f <workspace_root>/ai/agents/<actor-name>/inbox.fifo`
   - Remove the directory: `rm -rf <workspace_root>/ai/agents/<actor-name>`
4. **Re-launch** the actor if needed.

### Re-launching a Recovered Actor

```bash
mkdir -p "<workspace_root>/ai/agents/<actor-name>"
# Then re-start the actor as a delegate
```

## Message Format

Messages are plain text. Recommended format for structured communication
is JSON:

```json
{"command": "do_work", "params": {...}}
{"command": "report_status", "to": "coordinator", "status": "done"}
{"command": "shutdown"}
```

Format-agnostic — any text is valid.

### Data Passing: Send Content, Not Status Reports

When an actor completes work and writes back, **pass the actual data**
(file paths or content), not just a status message like "Story complete".

The recipient needs the data to do their job. A status message forces
them to go find files on disk, which may not exist in their filesystem
scope. Instead:

- **Include file paths** in response messages: `{"result": "wrote to /path/to/story.txt"}`
- **Or pass content inline** for small payloads: `{"result": "The actual story text..."}`
- **Or chain file paths** through the pipeline: editor tells writer to write
  to a known path, tells proofreader to read from that path and write to another.

The coordinator should either:
1. Read the output files themselves using filesystem tools, or
2. Instruct each worker to write to a predictable path and pass that path
   to the next worker in the chain.

## Coordinator Instruction Efficiency

When delegating a coordinator actor, keep instructions **concise** —
target 5-6 high-level steps, not 10+. Each step the coordinator reads
consumes a turn. A coordinator with 10+ steps often exhausts its
turn budget before completing the final step (e.g., writing the
compiled output).

**Good pattern (5-6 steps):**
1. Setup (mkdir + linger)
2. Wait for peers
3. Send work
4. Collect results
5. Compile output
6. Shutdown

**Bad pattern (10+ steps):**
Breaking each step into sub-steps with explicit polling loops, multiple
read-checks, and granular file operations. Trust the worker actors to
follow their own instructions.

The same applies to worker actors: give them the info they need
(role, workspace_root, what to do) but don't micromanage how they
do it. Trust the actor protocol.

## Output + Signal Pattern

When a worker actor completes work, prefer this two-part pattern:

1. **Write output to a file** in the worker's directory
2. **Signal completion** via the coordinator's inbox

This lets the coordinator collect results asynchronously without
blocking on file reads. The signal tells the coordinator "ready to
read" — the coordinator then reads the file.

```
# Worker side:
write output to <workspace_root>/ai/agents/<worker>/output.txt
synctell_write to <workspace_root>/ai/agents/<coordinator>/inbox.fifo: "<worker> done"

# Coordinator side:
# Wait for N signals, then read each output file
```

This pattern is more robust than having workers write data inline
in their messages, because file contents can be arbitrarily large.

## Stale FIFO Detection & Cleanup

When a delegate is killed or crashes without cleaning up, stale FIFOs
(`inbox.fifo.stale`, `inbox.fifo.old`) can accumulate. These can confuse
coordinators who check for FIFO existence as a liveness signal.

**Detection:** Before checking for a peer's FIFO, list the directory
and look for unexpected files:

```bash
ls -la <workspace_root>/ai/agents/<actor-name>/
# If you see .stale or .old files, the actor may have been restarted
```

**Cleanup:** Remove stale FIFOs before re-launching:

```bash
rm -f <workspace_root>/ai/agents/<actor-name>/*.stale
rm -f <workspace_root>/ai/agents/<actor-name>/*.old
```

**Prevention:** When starting fresh, always clean the whole agents
network first:

```bash
rm -rf <workspace_root>/ai/agents/
mkdir -p <workspace_root>/ai/agents/
```

## Zombie Process Detection

After cancelling or killing subagents, check for lingering processes:

```bash
ps aux | grep -E 'goose|synctell' | grep -v grep
```

`synctell read` processes should terminate on SIGTERM. If they don't,
use SIGKILL. The `synctell mcp` server process stays alive (that's the
daemon), but individual read/roundrobin/broadcast processes should clean
up.

## Important Notes

- **FIFOs are created by readers, not writers.** `synctell read` or
  `synctell_read_start_linger` creates the FIFO. Writers wait for it.
- **Linger is on by default.** The reader stays alive for multiple
  writers. No `-L` flag needed.
- **Blocking is synchronization.** Writers block until data is
  delivered. This provides natural backpressure.
- **The FIFO's existence is a signal.** If `inbox.fifo` exists, the
  actor is listening.
- **Clean up on exit.** `synctell read` handles FIFO cleanup on
  SIGTERM/SIGINT. Remove the directory entry afterward.
- **Filesystem sandboxing.** Delegates inherit the main agent's allowed
  filesystem scope. Use paths the main agent can access.
- **Wayward actors.** The orchestrator should monitor for stuck actors
  and SIGTERM → SIGKILL → cleanup → re-launch as needed.
- **High max_turns.** Give long-lived actors generous max_turns (~100+
  or more) so they can complete their listen→act→listen cycles without
  exhausting their action budget.
- **Startup race.** Actors start asynchronously. A coordinator actor
  MUST wait/poll for coworker FIFOs before sending messages, or writes
  will hang forever. Use the timeout pattern in Step 3b.
- **Check before write.** Always verify a FIFO exists before writing to
  it. A write to a non-existent FIFO blocks indefinitely.
- **Pass data, not just status.** Workers need actual content to work with.
  Send file paths or inline content, not just "done" messages.