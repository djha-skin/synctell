# synctell

A command-line utility for instant FIFO (named pipe) creation and communication.
`synctell` creates and interacts with POSIX FIFO special files, providing a
dead-simple, infrastructure-free interface for inter-process messaging.

## Installation

```bash
cargo install synctell
```

## Usage

```bash
# Write a message into a FIFO (creates it automatically)
synctell -o my-fifo "hello, world"

# Write stdin into a FIFO
echo "hello" | synctell -o my-fifo

# Read from a FIFO (must already exist and be a FIFO)
synctell -i my-fifo

# Write with a timeout — exit 124 if no reader connects in 5 seconds
synctell -o my-fifo -t 5 "important message"
```

## Examples

### Pipe between two shells

The writer must start first — it creates the FIFO and blocks until a reader
connects:

**Shell 1** (writer — start this first):
```bash
synctell -o my-fifo "the answer is 42"
```

The writer blocks, waiting for a reader. Now start the reader:

**Shell 2** (reader — start this second):
```bash
synctell -i my-fifo
```

Shell 2 prints `the answer is 42` and both shells return. The FIFO is
automatically removed after the write completes.

### With a timeout

```bash
# This will exit 124 after 3 seconds if nobody reads
synctell -o my-fifo -t 3 "are you there?"
```

### Chaining with other tools

Start the writer in the background so it creates the FIFO and blocks, then
run the reader in the foreground:

```bash
# Writer: buffer stdin into a FIFO (runs in background, blocks until reader connects)
cat big-file.csv | synctell -o data-pipe &

# Reader: consume the FIFO (runs in foreground, unblocks the writer)
synctell -i data-pipe | sort | uniq -c > result.txt

wait
```

Note: `synctell` buffers all of stdin into memory before creating the FIFO.
For very large files, be aware of the memory usage.

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

Agent A sends a message to Agent B. The writer creates the FIFO and blocks
until Agent B reads:

```bash
# Agent A — writer (starts first, creates FIFO, blocks until reader connects)
synctell -o agent-b/inbox.fifo "task complete: step 3 done"
```

Agent B picks up the message:

```bash
# Agent B — reader (starts second, opens existing FIFO)
synctell -i agent-b/inbox.fifo
# prints: task complete: step 3 done
```

### Why this works so well for agents

**1. Blocking is synchronization.** When Agent A runs `synctell -o inbox.fifo "message"`,
it creates the FIFO and then **blocks** — hangs — until Agent B opens the
other end with `synctell -i inbox.fifo`. No polling, no busy-waiting, no
wasted CPU. The OS handles the rendezvous. The writer cannot proceed until
the reader connects, providing natural flow control.

**2. One-shot message passing.** Each FIFO carries exactly one message. The
writer creates the FIFO, writes, and removes it. This eliminates
complexities of shared state, message queues, or buffer management. If
Agent A needs to send another message, it creates a new FIFO. Simple and
predictable.

**3. No infrastructure required.** No message queue to install. No broker to
configure. No network socket to bind. A FIFO is a file. It lives in the
filesystem, visible to every agent that has directory access. You can `ls`
it, `stat` it, `rm` it. It is as simple as messaging gets.

**4. Predictable lifecycle.** The FIFO's existence signals that a message is
incoming. Once the writer creates it, it exists until the write completes
and `synctell` removes it. Agents can coordinate by agreeing on FIFO
paths — the path itself is the protocol.

**5. Timeout for liveness.** Use `-t` to avoid deadlocks. If the expected
reader never shows up, `synctell` exits with code 124 instead of hanging
forever. Your orchestration layer can detect this and re-route work.

### Example: multi-agent pipeline

Each step creates a one-shot FIFO for the next. The writer must start first —
it creates the FIFO and blocks until the reader connects:

```bash
# Step 1 → Step 2: Agent A sends a message, Agent B receives it
synctell -o pipeline/step2-input.fifo "processed: image-0042.png" &
synctell -i pipeline/step2-input.fifo | process-image > /tmp/step2-output.bin
wait

# Step 2 → Step 3: Agent B sends its result, Agent C receives it
cat /tmp/step2-output.bin | synctell -o pipeline/step3-input.fifo &
synctell -i pipeline/step3-input.fifo | send-to-storage
wait
```

Each `-o` creates a FIFO, writes, and removes it. Each `-i` reads from an
existing FIFO and streams to stdout. Run the writer (background) before the
reader (foreground) so the FIFO exists when the reader checks for it.

## How It Works

`synctell` uses the `mkfifo(3)` system call to create POSIX named pipes. The
blocking open semantics of FIFOs (a write-open blocks until a reader opens
the other end, and vice versa) provide natural synchronization without
additional coordination.

In output mode (`-o`), `synctell` first buffers all of stdin (if no positional
message is given) into memory **before** creating the FIFO. This prevents
deadlock: if the FIFO were opened before reading stdin, the pipe buffer could
fill while blocked waiting for a reader. For very large inputs, be aware that
the entire input is held in memory.

When a timeout is specified (`-t`), `synctell` spawns a background thread to
handle the blocking FIFO open, while the main thread waits with a deadline.
If the deadline expires, the FIFO is cleaned up and the process exits with
code **124**.

After a successful write, `synctell` removes the FIFO from the filesystem,
keeping your working directory clean. Each `-o` call is one-shot: create,
write, remove.

## Exit Codes

| Code | Meaning |
|------|---------|
| 0    | Success |
| 1    | General error (missing arguments, write failure, etc.) |
| 124  | Timeout — no reader connected within the specified duration |

## License

MIT
