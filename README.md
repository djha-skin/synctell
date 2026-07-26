# synctell

A command-line utility for instant FIFO (named pipe) creation and communication.
`synctell` creates and interacts with POSIX FIFO special files, providing a
dead-simple, infrastructure-free interface for inter-process messaging.

Readers (`read`) create the FIFO and clean it up when done. Writers (`write`) poll
for the FIFO and stream data into it. You never need to manage FIFO lifecycle
by hand. Because readers create the FIFOs, multiple writers can write to the same
reader -- and the FIFO's presence on disk is a clean signal that someone
is listening for a message.

## Installation

```bash
cargo install synctell
```

## Commands

| Command       | Description |
|---------------|-------------|
| `synctell read`  | Create a FIFO and read messages from writers |
| `synctell write` | Poll for a FIFO and write a message to it |
| `synctell broadcast` | Read from one FIFO, write to many |
| `synctell mcp`   | Start an MCP server over stdio |

## Usage

### Read

```bash
# Read a single message from a FIFO (creates it, reads one message, exits)
synctell read my-fifo

# Read many messages — stay alive for multiple writers until SIGINT
synctell read -l my-fifo

# Read with a timeout — exit 124 if no writer connects in 5 seconds
synctell read -t 5 my-fifo

# Read with a hard max-time — exit 0 after 10 seconds no matter what
synctell read -m 10 my-fifo

# Linger with max-time — receive messages for 10 seconds, then exit cleanly
synctell read -l -m 10 my-fifo
```

### Write

```bash
# Write a message into a FIFO (waits for a reader to appear, writes, then exits)
synctell write my-fifo "hello, world"

# Write stdin into a FIFO
echo "hello" | synctell write my-fifo

# Write with a timeout — exit 124 if no reader appears in 5 seconds
synctell write -t 5 my-fifo "important message"
```

### Broadcast

```bash
# Broadcast: read from one FIFO, fan out to multiple outputs
# Linger is automatic on the input — stays alive for multiple writers.
# Output FIFOs must already exist (created by their readers first).
synctell broadcast input.fifo output1.fifo output2.fifo output3.fifo

# Broadcast with timeout — exit 124 if no writer on input within 5 seconds
synctell broadcast -t 5 input.fifo output1.fifo output2.fifo

# Broadcast with max-time — exit 0 after 30 seconds even if messages still flowing
synctell broadcast -m 30 input.fifo output1.fifo
```

### Flag reference

| Flag | Applies to | Description |
|------|-----------|-------------|
| `-l` / `--linger` | read | Keep reading after the first message (stay alive for multiple writers) |
| `-t` / `--timeout` SECS | read, write, broadcast | Wait at most N seconds for the first peer. Exit 124 if none. Discarded once first message arrives. 0 = block forever. |
| `-m` / `--max-time` SECS | read, broadcast | Hard deadline — quit after N seconds no matter what. Never discarded. Exit 0. 0 = no limit. |

## Examples

### Pipe between two shells

The reader (`read`) creates the FIFO and reads one message, then exits.
The writer (`write`) waits for the FIFO to appear, opens it, writes, and exits.

**Shell 1** (reader — start this first, or concurrently with `-t` on the
writer):
```bash
synctell read my-fifo
```

**Shell 2** (writer — the FIFO already exists, so this returns immediately):
```bash
synctell write my-fifo "the answer is 42"
```

Shell 1 prints `the answer is 42` and exits. The writer exits as soon as its
message is delivered.

To accept **multiple** messages from different writers, add `-l`:

```bash
synctell read -l my-fifo
```

> **Note:** Without a timeout (`-t`), the writer exits immediately with
> code 1 if the FIFO is not yet present. If you're unsure which side
> starts first, give the writer a timeout:
>
> ```bash
> synctell write -t 10 my-fifo "I'll wait up to 10 seconds"
> ```

### With a timeout

```bash
# Writer: exit 124 after 3 seconds if no reader is listening
synctell write -t 3 my-fifo "are you there?"

# Reader: exit 124 after 3 seconds if no writer shows up.
# Without -l, exits after the first message.
synctell read -t 3 my-fifo

# Reader (linger mode): exit 124 after 3 seconds if no writer shows up.
# Once one writer has connected, the reader stays alive indefinitely.
synctell read -l -t 3 my-fifo
```

### With max-time

```bash
# Reader: exit 0 after 10 seconds, even if no writer connects
synctell read -m 10 my-fifo

# Reader (linger): receive messages for up to 10 seconds, then exit cleanly
synctell read -l -m 10 my-fifo

# Broadcast: fan out messages for up to 30 seconds, then exit cleanly
synctell broadcast -m 30 input.fifo output1.fifo output2.fifo
```

### Multiple writers, one reader

A single reader with `-l` accepts messages from any number of writers.
Each message arrives as a separate chunk on the reader's stdout. If a
writer's data doesn't end with a newline, the reader appends one — so
writers that send plain messages line up neatly on the receiver's output:

```bash
# Terminal 1: one reader, listening until interrupted
synctell read -l inbox.fifo

# Terminal 2, 3, 4: many writers, each delivering a message
synctell write inbox.fifo "from agent-a"
synctell write inbox.fifo "from agent-b"
synctell write inbox.fifo "from agent-c"
```

Terminal 1 prints:
```
from agent-a
from agent-b
from agent-c
```
Then it waits for the next writer. Ctrl-C to clean up.

> If you need writers to ship multi-line payloads or binary data, end
> each message with a newline yourself and the reader won't add one.

### Broadcast: one input, many outputs

The broadcast subcommand reads from one input FIFO and writes each message
to every output FIFO. Linger is automatic — the broadcast stays alive for
multiple writers. Output FIFOs must already exist (their readers created them).

```bash
# Terminal 1: output reader A
synctell read -l output-a.fifo

# Terminal 2: output reader B
synctell read -l output-b.fifo

# Terminal 3: broadcast (must start after output FIFOs exist)
synctell broadcast input.fifo output-a.fifo output-b.fifo

# Terminal 4: any number of writers send to the input
synctell write input.fifo "message for everyone"
synctell write input.fifo "another broadcast"
```

Both output readers receive every message.

### With a hard deadline (max-time)

```bash
# Broadcast for up to 60 seconds, then exit cleanly.
# Even if messages are still flowing, -m is a hard deadline.
synctell broadcast -m 60 input.fifo output-a.fifo output-b.fifo
```

This is useful in cron jobs, CI pipelines, or any scenario where you
want to ensure the process doesn't run indefinitely.

### Chaining with other tools

Start the writer in the background — it waits for the FIFO to appear
(poll-once per second) and unblocks as soon as the reader connects:

```bash
# Writer: buffer stdin into a FIFO (waits for a reader, then delivers)
cat big-file.csv | synctell write data-pipe &

# Reader: creates the FIFO, consumes stdin from each writer, streams to stdout
synctell read -l data-pipe | sort | uniq -c > result.txt

wait
```

Note: `synctell` buffers all of stdin into memory before opening the
FIFO for writing. This prevents deadlock: if the FIFO were opened
before reading stdin, the pipe buffer could fill while blocked waiting
for a reader. For very large files, be aware of the memory usage.

## Why FIFOs?

FIFOs are one of the oldest and most reliable IPC mechanisms on POSIX systems.
They require no daemons, no sockets, no shared memory — just a special file
that blocks until both a reader and a writer are connected. This blocking
behavior is not a limitation; it is the feature.

## AI Agent Communication

FIFOs are a natural fit for **AI agent-to-agent communication**. In a
multi-agent system, agents need a way to exchange messages, coordinate work,
and synchronize without a central broker. `synctell` makes this trivial.

### The pattern

```
agent-a/               agent-b/
  outbox.fifo            inbox.fifo
```

### Reader-driven semantics

In `synctell`, **readers** create the FIFOs they listen on, and **writers**
deliver to existing FIFOs. This has two useful properties:

1. **The FIFO's existence on disk is a clean signal.** If `inbox.fifo`
   is present, an agent is listening. That is much more interesting than
   the inverse — "an agent has something to say." Listening is the
   scarce resource; speaking is cheap.

2. **Many writers can write to one reader.** Each writer connects,
   delivers one message, and disconnects. The reader handles them
   sequentially. No broker, no port allocation, no shared state.

### A simple handoff

```bash
# Agent B — reader (creates inbox.fifo, listens for messages)
synctell read -l agent-b/inbox.fifo
```

```bash
# Agent A — writer (delivers a message; FIFO must already exist)
synctell write agent-b/inbox.fifo "task complete: step 3 done"
```

The reader prints `task complete: step 3 done` and continues listening.

### Why this works so well for agents

**1. Blocking is synchronization.** When Agent A runs
`synctell write inbox.fifo "message"`, it polls for the FIFO and **blocks**
until the FIFO appears (or its `-t` timeout expires). The writer cannot
deliver until the reader has created the FIFO, providing natural flow
control. No busy-waiting, no wasted CPU. The OS handles the rendezvous.
On the reader side, the 1-second poll interval keeps overhead minimal.

**2. Many-to-one messaging.** A single reader can accept messages from
any number of writers. Each writer connects, writes, and disconnects;
the reader accepts them sequentially and streams them to its stdout.
No queue management, no port conflicts, no broker to maintain.

**3. Presence is a signal.** Other agents can probe the filesystem to
discover who is listening. `ls agent-b/inbox.fifo` answers the question
"is Agent B ready to receive work?" without coordination. That is the
single most useful piece of state in any agent system: *someone is
home to answer*.

**4. No infrastructure required.** No message queue to install. No broker to
configure. No network socket to bind. A FIFO is a file. It lives in the
filesystem, visible to every agent that has directory access. You can
`ls` it, `stat` it, `rm` it. It is as simple as messaging gets.

**5. Predictable lifecycle.** The reader creates the FIFO, accepts
messages, and removes the FIFO when it exits. With `-l`, it stays alive
for multiple writers; without `-l`, it exits after the first message.
Writers poll for the FIFO, write, and exit. Agents can coordinate by
agreeing on FIFO paths — the path itself is the protocol.

**6. Timeout for liveness.** Use `-t` to avoid deadlocks. If the expected
peer never shows up, `synctell` exits with code 124 instead of hanging
forever. Your orchestration layer can detect this and re-route work.

### Example: multi-agent pipeline

Each step creates a one-shot reader for the next. The reader creates the
FIFO; the next step's writer polls for it:

```bash
# Step 1 → Step 2: Agent A reads input, Agent B consumes Agent A's output.
# (Some upstream producer writes the initial message via stdin or a writer.)
synctell read pipeline/step2-input.fifo | process-image > /tmp/step2-output.bin &

# Step 2 → Step 3: Agent B's output is delivered to Agent C.
cat /tmp/step2-output.bin | synctell write pipeline/step3-input.fifo &
synctell read pipeline/step3-input.fifo | send-to-storage
wait
```

Each `read` creates a FIFO, reads input, and removes the FIFO on exit.
With `-l`, a reader stays alive for multiple writers; without `-l`,
it exits after the first message.
Each `write` polls for the FIFO, writes, exits. The polling writer blocks
until the upstream reader has created the FIFO, providing natural
backpressure between stages.

### Example: fan-in (many writers → one reader)

Many agents reporting into a single observer:

```bash
# Observer: one reader, accepts reports from any number of agents
synctell read -l reports.fifo | tee -a /var/log/agents.log &
```

```bash
# Anywhere, any time: an agent drops a report into the observer
synctell write reports.fifo "$(hostname): step done"
synctell write reports.fifo "$(hostname): step done"
synctell write reports.fifo "$(hostname): step done"
```

The observer's log grows line by line, with each report arriving as
soon as its writer connects. No need for a broker, a log-collector
daemon, or a network socket — just a FIFO.

> **Note:** Without `-l`, the reader exits after the first report.
> Use `-l` when you expect multiple writers to report over time.

### Example: fan-out (one input → many outputs)

Use `synctell broadcast` to distribute a message to multiple agents:

```bash
# Agent A: broadcast listener for pipeline updates
synctell read -l build-status-a.fifo | process-status

# Agent B: another broadcast listener
synctell read -l build-status-b.fifo | process-status

# Coordinator: broadcast to all listeners
synctell broadcast input.fifo build-status-a.fifo build-status-b.fifo
```

Each output FIFO must be created by its reader before the broadcast
starts. The broadcast stays alive for multiple messages (linger is
automatic on its input).

## How It Works

`synctell` uses the `mkfifo(3)` system call to create POSIX named pipes
(in read/broadcast modes only). The blocking open semantics of FIFOs
(a write-open blocks until a reader opens the other end, and vice versa)
provide natural synchronization without additional coordination.

### Read mode

In read mode (`synctell read`), `synctell` calls `mkfifo(3)` once at startup,
then reads messages. For each iteration it blocks on `open(path)` for
reading, which returns only once a writer has connected; it reads the
message to EOF, writes the bytes (plus a trailing newline if the writer's
data didn't already end with one) to stdout, and then:

- **Without `-l`** (default): exits after the first message.
- **With `-l`**: goes back to waiting for the next writer, discarding any
  timeout. This continues until a SIGINT or SIGTERM sets a shutdown flag
  that the read loop checks once per second.

### Write mode

In write mode (`synctell write`), `synctell` first buffers all of stdin (if no
positional message is given) into memory **before** opening the FIFO.
This prevents deadlock: if the FIFO were opened before reading stdin,
the pipe buffer could fill while blocked waiting for a reader. For very
large inputs, be aware that the entire input is held in memory.

### Broadcast mode

In broadcast mode (`synctell broadcast`), `synctell` creates an input FIFO
(a la `read`) and spawns a reader thread that loops on blocking
`open()` → `read_to_end()` → send to channel. The main loop receives
messages from the channel and fans them out to all output FIFOs using
detached writer threads (one per output). Linger is implicit — the
broadcast stays alive for multiple writers. If an output FIFO's reader
disappears, the broadcast logs a warning and continues to remaining
outputs.

### Timeout and max-time

When a timeout is specified (`-t`), write mode polls once per second
for the FIFO's existence. If the FIFO has not appeared by the deadline,
the process exits with code **124**. Read mode, on timeout without any
writer ever connecting, also removes the FIFO it created and exits with
code **124**. In linger mode (`-l`), once a reader has received at least
one message, it stays alive indefinitely — the timeout only governs the
wait for the *first* writer.

When max-time is specified (`-m`), it acts as a hard deadline. Unlike
timeout, it is never discarded — the process exits cleanly with code **0**
after the specified number of seconds, even if messages are still flowing.
This is useful for cron jobs, CI pipelines, and any scenario where the
process must not run indefinitely.

### FIFO lifecycle

The reader removes the FIFO from the filesystem when it exits (whether
cleanly via SIGINT/SIGTERM, by timeout, or by max-time). Each `read` or
`broadcast` invocation owns its input FIFO for its entire lifetime;
writers come and go freely.

## MCP Server

`synctell` includes a built-in MCP (Model Context Protocol) server that
exposes its FIFO operations as tools over JSON-RPC on stdio. This lets
AI agents — or any MCP-compatible host — create, read, write, and
broadcast FIFOs programmatically.

### Starting the server

```bash
synctell mcp
```

The server speaks the MCP protocol on stdin/stdout. It is designed to be
launched by an MCP host (such as a goose session or any MCP-capable
agent framework).

### Available tools

| Tool | Description | Parameters |
|------|-------------|------------|
| `synctell_write` | Write a message to a FIFO | `path` (string), `message` (string) |
| `synctell_read_oneshot` | Create a FIFO, read one message, remove FIFO | `path` (string), `timeout` (integer, optional, 0 = block forever) |
| `synctell_read_start_linger` | Create a FIFO, start a background reader accepting multiple writers | `path` (string), `timeout` (integer, optional, accepted but not yet enforced) |
| `synctell_read_still_linger` | Read the next message from an active linger reader without stopping it | `path` (string), `timeout` (integer, optional, 0 = block forever) |
| `synctell_read_stop_linger` | Stop a lingering reader, return buffered data | `path` (string) |
| `synctell_broadcast` | Broadcast a message from one FIFO to multiple output FIFOs | `path` (string), `outputs` (string array), `timeout` (integer, optional), `max_time` (integer, optional) |

### Timeout semantics

The `timeout` parameter on `synctell_read_oneshot`, `synctell_read_still_linger`, and `synctell_broadcast` accepts a non-negative integer in seconds. A value of `0` (the default) means **block forever** — the tool waits indefinitely for a peer or message. A positive value means "return an error if no peer (or message) appears within N seconds."

The `max_time` parameter on `synctell_broadcast` accepts a non-negative
integer in seconds. A value of `0` (the default) means no limit. A
positive value means "exit cleanly after N seconds no matter what."

> **Note:** The `timeout` parameter on `synctell_read_start_linger` is
> accepted for forward compatibility but is not yet enforced. The linger
> reader always blocks until a writer connects or the reader is stopped.

### Example: configuring in a goose session

```yaml
extensions:
  synctell:
    name: synctell
    command: synctell mcp
```

### Lifecycle

1. Call `synctell_read_start_linger` (or `synctell_read_oneshot`) to
   create a FIFO and start listening.
2. The FIFO's presence on disk signals that a reader is active.
3. Call `synctell_write` to deliver messages to the FIFO.
4. (Optional) Call `synctell_read_still_linger` to read the next message
   without stopping the reader. Repeat as needed for each message.
5. Call `synctell_read_stop_linger` to collect all remaining buffered
   messages and clean up.

For one-shot communication, `synctell_read_oneshot` handles the full
lifecycle: create FIFO → read one message → remove FIFO — in a single
call.

### Broadcast example

1. Create output FIFOs using `synctell_read_start_linger` for each
   broadcast receiver.
2. Call `synctell_broadcast` with the input path and output path list.
3. Use `synctell_write` to deliver messages to the broadcast input.
4. The broadcast fans out each message to all output FIFOs.
5. Stop linger readers with `synctell_read_stop_linger`.

## Exit Codes

| Code | Meaning |
|------|---------|
| 0    | Success |
| 1    | General error (missing arguments, FIFO not present for writer, etc.) |
| 124  | Timeout — the expected peer did not appear within the specified duration |

## License

MIT