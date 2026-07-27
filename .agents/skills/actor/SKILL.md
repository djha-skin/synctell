---
name: actor
description: >
  Become an actor in a multi-agent work network. Opens a mailbox (FIFO) at
  `ai/agents/<actor-name>` and processes messages in a listen→act→listen
  loop until instructed to terminate. Coworkers are discovered at
  `ai/agents/` in the current working directory.
---

# Actor Skill

When you are told to use this skill, you become an **actor** in a
multi-agent work network. Actors communicate through FIFO mailboxes
(using `synctell`). Each actor has a personal mailbox; all mailboxes
live under `ai/agents/` in the current working directory.

## Protocol Overview

```
ai/agents/
  ├── coordinator/  ← mailbox for the coordinator
  ├── worker-a/     ← mailbox for worker-a
  └── worker-b/     ← mailbox for worker-b
```

Each actor's mailbox FIFO is at `ai/agents/<actor-name>/inbox.fifo`.
The presence of a FIFO signals that the actor is listening.

## Actor Identity

- You are told your **actor name** (e.g., `"worker-a"`, `"coordinator"`).
  This is the identity you use for the duration of the session.
- Your mailbox FIFO path is `ai/agents/<actor-name>/inbox.fifo`.
- All peer mailboxes are discovered by scanning `ai/agents/` for
  subdirectories containing `inbox.fifo` files.
- All FIFO paths are relative to the **current working directory** of the
  agent process.

## Activation Protocol

When you are asked to "become an actor" or "use the actor skill":

### Step 1 — Create the mailbox directory

```bash
mkdir -p "ai/agents/<actor-name>"
```

### Step 2 — Start a linger reader on your mailbox FIFO

```bash
synctell read ai/agents/<actor-name>/inbox.fifo &  # linger reader in background
```

**Important:** The reader runs in the background. It creates the FIFO,
stays alive for multiple writers, and prints each received message to
stdout (which you must capture and process).

### Step 3 — Announce your presence

Log a message that you are listening. Optionally notify any coordinator
or other known actors that you are ready.

### Step 4 — Listen → Act → Listen loop

Repeatedly:
1. **Read the next message** from your mailbox FIFO.
2. **Parse the message.** Messages are text — typically a JSON payload
   or a structured command. The expected format depends on the
   orchestration layer (coordinator, master agent, etc.).
3. **Act on the message.** Perform the requested work. This may involve
   reading from or writing to other actors' mailboxes.
4. **Go back to listening** immediately after completing the work.

### Step 5 — Termination

When you receive a **termination message** on your mailbox (a message
whose content signals "shutdown", typically a JSON object with
`{"command": "shutdown"}` or similar), you:

1. Stop the linger reader (kill the background `synctell read` process).
2. Remove the FIFO: `rm -f ai/agents/<actor-name>/inbox.fifo`.
3. Exit cleanly.

## Messaging Coworkers

To send a message to a coworker, write to their mailbox FIFO:

```bash
synctell write ai/agents/<coworker-name>/inbox.fifo "your message here"
```

The write will block (with optional `-t` timeout) until the recipient's
FIFO exists. If the FIFO doesn't exist, the coworker is not listening
and the message cannot be delivered.

## Discovering Coworkers

Scan the `ai/agents/` directory to discover who is listening:

```bash
ls -d ai/agents/*/inbox.fifo 2>/dev/null
```

Each entry in the output is a live mailbox. The directory name is the
actor name.

## Orchestration Patterns

The Actor skill is designed to be used by subagents that are spun up by
a master agent. The master agent uses `synctell` constructions to
coordinate work:

### Broadcast (fan-out)

The master agent creates a broadcast to send the same message to all
actors:

```bash
# Master: start broadcast
synctell broadcast instructions.fifo $(ls -d ai/agents/*/inbox.fifo | tr '\n' ' ')
```

```bash
# Master: write instructions
synctell write instructions.fifo "{\"task\": \"process dataset\"}"
```

### Round-robin (load balancing)

The master agent distributes work items evenly across actors:

```bash
# Master: start round-robin
synctell roundrobin tasks.fifo ai/agents/worker-a/inbox.fifo ai/agents/worker-b/inbox.fifo ai/agents/worker-c/inbox.fifo
```

```bash
# Master: submit tasks one at a time
synctell write tasks.fifo "{\"task\": \"item-1\"}"
synctell write tasks.fifo "{\"task\": \"item-2\"}"
synctell write tasks.fifo "{\"task\": \"item-3\"}"
```

### Peer-to-peer

Actors can write directly to each other:

```bash
# Inside worker-a: send a result to worker-b
synctell write ai/agents/worker-b/inbox.fifo "{\"from\": \"worker-a\", \"result\": \"done\"}"
```

## Message Format

Messages are plain text. The recommended format for structured
communication is JSON:

```json
{"command": "do_work", "params": {...}}
{"command": "report_status", "to": "coordinator", "status": "done"}
{"command": "shutdown"}
```

However, the Actor skill is format-agnostic — any text that your agent
can parse and act on is valid.

## Important Notes

- **FIFOs are created by readers, not writers.** The `synctell read`
  command creates the FIFO. Writers (`synctell write`) wait for it to
  appear.
- **Linger is on by default.** The reader stays alive for multiple
  writers. You do not need the `-L` flag.
- **Blocking is synchronization.** When you write to a FIFO, you block
  until the data is delivered. This provides natural backpressure.
- **The FIFO's existence is a signal.** If `inbox.fifo` exists, the
  actor is listening. You can check with `test -f`.
- **Clean up on exit.** Always remove your mailbox FIFO when you shut
  down, so other actors know you are no longer available.