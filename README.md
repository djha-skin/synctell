# synctell

A command-line utility for instant FIFO (named pipe) creation and communication.
`synctell` creates and interacts with POSIX FIFO special files, providing a
dead-simple, infrastructure-free interface for inter-process messaging.

Writers (`-o`) create the FIFO and clean it up when done. Readers (`-i`) poll
for the FIFO and stream it to stdout. You never need to manage FIFO lifecycle by
hand.

## Installation

```bash
cargo install synctell
```

## Usage

```bash
# Write a message into a FIFO (creates it, blocks until reader connects, then removes it)
synctell -o my-fifo "hello, world"

# Write stdin into a FIFO
echo "hello" | synctell -o my-fifo

# Read from a FIFO (polls for it to appear, then reads; does NOT create it)
synctell -i my-fifo

# Write with a timeout — exit 124 if no reader connects in 5 seconds
synctell -o my-fifo -t 5 "important message"

# Read with a timeout — exit 124 if the file doesn't appear in 5 seconds
synctell -i my-fifo -t 5
```

## Examples

### Pipe between two shells

The writer (`-o`) creates the FIFO and blocks until a reader opens the other
end. The reader (`-i`) polls for the FIFO to appear, then opens it — which
unblocks the writer:

**Shell 1** (writer — start first, or concurrently with `-t` on the reader):
```bash
synctell -o my-fifo "the answer is 42"
```

**Shell 2** (reader):
```bash
synctell -i my-fifo
```

Shell 2 prints `the answer is 42` and both shells return. The writer
automatically removes the FIFO after the write completes.

> **Note:** Without a timeout (`-t`), the reader must find the FIFO already
> present or it exits immediately with code 1. If you're unsure which side
> starts first, give the reader a timeout:
>
> ```bash
> synctell -i my-fifo -t 10
> ```

### With a timeout

```bash
# Exit 124 after 3 seconds if no reader connects
synctell -o my-fifo -t 3 "are you there?"

# Exit 124 after 3 seconds if the FIFO doesn't appear
synctell -i my-fifo -t 3
```

### Chaining with other tools

Start the writer in the background — it creates the FIFO and blocks until a
reader connects. Then run the reader in the foreground — it polls for the
FIFO, opens it (unblocking the writer), and streams it to stdout:

```bash
# Writer: buffer stdin into a FIFO (runs in background, blocks until reader connects)
cat big-file.csv | synctell -o data-pipe &

# Reader: poll for the FIFO, then consume it (runs in foreground)
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
until a reader opens the other end:

```bash
# Agent A — writer (creates FIFO, blocks until reader connects)
synctell -o agent-b/inbox.fifo "task complete: step 3 done"
```

Agent B picks up the message. It polls for the FIFO, then opens it — which
unblocks the writer:

```bash
# Agent B — reader (polls for FIFO, then reads it)
synctell -i agent-b/inbox.fifo
# prints: task complete: step 3 done
```

### Why this works so well for agents

**1. Blocking is synchronization.** When Agent A runs `synctell -o inbox.fifo "message"`,
it creates the FIFO and then **blocks** — hangs — until Agent B opens the
other end with `synctell -i inbox.fifo`. The writer cannot proceed until the
reader opens the FIFO, providing natural flow control. No busy-waiting, no
wasted CPU on the writer side. The OS handles the rendezvous. On the reader
side, the poll interval (1 second by default) keeps overhead minimal.

**2. One-shot message passing.** Each FIFO carries exactly one message. The
writer creates the FIFO, writes, and removes it. This eliminates
complexities of shared state, message queues, or buffer management. If
Agent A needs to send another message, it creates a new FIFO. Simple and
predictable.

**3. No infrastructure required.** No message queue to install. No broker to
configure. No network socket to bind. A FIFO is a file. It lives in the
filesystem, visible to every agent that has directory access. You can `ls`
it, `stat` it, `rm` it. It is as simple as messaging gets.

**4. Predictable lifecycle.** The writer (`-o`) creates the FIFO, writes, and
removes it. The reader (`-i`) polls for the FIFO and reads it. Agents can
coordinate by agreeing on FIFO paths — the path itself is the protocol.

**5. Timeout for liveness.** Use `-t` to avoid deadlocks. If the expected
peer never shows up, `synctell` exits with code 124 instead of hanging
forever. Your orchestration layer can detect this and re-route work.

### Example: multi-agent pipeline

Each step creates a one-shot FIFO for the next. The writer goes first — it
creates the FIFO and blocks until the reader connects:

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

Each `-o` creates a FIFO, writes, and removes it. Each `-i` polls for the
FIFO, reads, and streams to stdout. The background writer blocks until the
foreground reader connects.

## How It Works

`synctell` uses the `mkfifo(3)` system call to create POSIX named pipes (in
output mode only). The blocking open semantics of FIFOs (a write-open blocks
until a reader opens the other end, and vice versa) provide natural
synchronization without additional coordination.

In output mode (`-o`), `synctell` first buffers all of stdin (if no positional
message is given) into memory **before** creating the FIFO. This prevents
deadlock: if the FIFO were opened before reading stdin, the pipe buffer could
fill while blocked waiting for a reader. For very large inputs, be aware that
the entire input is held in memory.

When a timeout is specified (`-t`), `synctell` spawns a background thread to
handle the blocking FIFO open, while the main thread waits with a deadline.
If the deadline expires, the FIFO is cleaned up and the process exits with
code **124**.

In input mode (`-i`), `synctell` polls the filesystem for the target file.
Without a timeout, it checks once and exits immediately if the file is absent.
With a timeout, it checks every second. Once the file appears, it opens it
and streams its contents to stdout. The reader does not create or remove any
files — that is the writer's job.

After a successful write, the writer removes the FIFO from the filesystem,
keeping your working directory clean. Each `-o` invocation is one-shot:
create, write, remove.

## Exit Codes

| Code | Meaning |
|------|---------|
| 0    | Success |
| 1    | General error (missing arguments, file not found, write failure, etc.) |
| 124  | Timeout — the expected peer or file did not appear within the specified duration |

## License

MIT
