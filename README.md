# tell

A command-line utility for instant FIFO (named pipe) creation and communication.
`tell` creates and interacts with POSIX FIFO special files, providing a
zero-dependency, dead-simple interface for inter-process messaging.

## Installation

```bash
cargo install tell
```

## Usage

```bash
# Write a message into a FIFO (creates it automatically)
tell -o my-fifo "hello, world"

# Write stdin into a FIFO
echo "hello" | tell -o my-fifo

# Read from a FIFO (must already exist and be a FIFO)
tell -i my-fifo

# Write with a timeout — exit 124 if no reader connects in 5 seconds
tell -o my-fifo -t 5 "important message"
```

## Examples

### Pipe between two shells

**Shell 1** (reader):
```bash
tell -i my-fifo
```

**Shell 2** (writer):
```bash
tell -o my-fifo "the answer is 42"
```

Shell 1 prints `the answer is 42` and both shells return.

### With a timeout

```bash
# This will exit 124 after 3 seconds if nobody reads
tell -o my-fifo -t 3 "are you there?"
```

### Chaining with other tools

```bash
# Writer: generate data into a FIFO
cat big-file.csv | tell -o data-pipe

# Reader: consume it elsewhere
tell -i data-pipe | sort | uniq -c > result.txt
```

## Why FIFOs?

FIFOs are one of the oldest and most reliable IPC mechanisms on POSIX systems.
They require no daemons, no sockets, no shared memory — just a special file
that blocks until both a reader and a writer are connected. This blocking
behavior is not a limitation; it is the feature.

## AI Agent Communication

FIFOs are a natural fit for **AI agent-to-agent communication**. In a
multi-agent system, agents need a way to exchange messages, coordinate work,
and synchronize without a central broker. `tell` makes this trivial.

### The pattern

```
agent-a/               agent-b/
  inbox.fifo             inbox.fifo
```

Agent A wants to send a message to Agent B:

```bash
tell -o agent-b/inbox.fifo "task complete: step 3 done"
```

Agent B, waiting for work:

```bash
tell -i agent-b/inbox.fifo
# prints: task complete: step 3 done
```

### Why this works so well for agents

**1. Blocking is synchronization.** When Agent B runs `tell -i inbox.fifo`,
it hangs — blocks — until Agent A writes. No polling, no busy-waiting, no
wasted CPU. The OS handles the rendezvous. This is a **semaphore**: the
reader cannot proceed until a message arrives.

**2. FIFOs are mutexes.** Only one writer can be connected at a time for
meaningful message delivery. If Agent A is writing, Agent C must wait. This
gives you **mutual exclusion** for free — no lock files, no PID files, no
race conditions.

**3. No infrastructure required.** No message queue to install. No broker to
configure. No network socket to bind. A FIFO is a file. It lives in the
filesystem, visible to every agent that has directory access. You can `ls`
it, `stat` it, `rm` it. It is as simple as messaging gets.

**4. Observable state.** A FIFO's existence in the filesystem *is* the state.
If `inbox.fifo` exists, agents know where to send messages. If it does not
exist, the channel is closed. No extra discovery protocol needed.

**5. Timeout for liveness.** Use `-t` to avoid deadlocks. If an expected
agent never shows up, `tell` exits with code 124 instead of hanging
forever. Your orchestration layer can detect this and re-route work.

### Example: multi-agent pipeline

```bash
# Agent A produces a result
tell -o pipeline/step2-input.fifo "processed: image-0042.png"

# Agent B picks it up, processes, hands off to Agent C
tell -i pipeline/step2-input.fifo | process-image | tell -o pipeline/step3-input.fifo

# Agent C consumes the final output
tell -i pipeline/step3-input.fifo | send-to-storage
```

Each agent is an independent process. The FIFOs are the edges in a DAG. The
agents are the nodes. `tell` is the only interface you need.

## How It Works

`tell` uses the `mkfifo(3)` system call to create POSIX named pipes. The
blocking open semantics of FIFOs (a write-open blocks until a reader opens
the other end, and vice versa) provide natural synchronization without
additional coordination.

When a timeout is specified (`-t`), `tell` spawns a background thread to
handle the blocking FIFO open, while the main thread waits with a deadline.
If the deadline expires, the FIFO is cleaned up and the process exits with
code **124**.

After a successful write, `tell` removes the FIFO from the filesystem,
keeping your working directory clean.

## Exit Codes

| Code | Meaning |
|------|---------|
| 0    | Success |
| 1    | General error (missing arguments, write failure, etc.) |
| 124  | Timeout — no reader connected within the specified duration |

## License

MIT
