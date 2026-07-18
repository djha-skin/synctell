# synctell

A command-line utility for instant FIFO (named pipe) creation and communication.
`synctell` creates and interacts with POSIX FIFO special files, providing a
dead-simple, infrastructure-free interface for inter-process messaging.

Readers (`-i`) create the FIFO and clean it up when done. Writers (`-o`) poll
for the FIFO and stream data into it. You never need to manage FIFO lifecycle
by hand. Because readers create the FIFOs, multiple writers can write to the
same reader — and the FIFO's presence on disk is a clean signal that *someone
is listening*, which is more useful than "someone wants to say something."

## Installation

```bash
cargo install synctell
```

## Usage

```bash
# Read from a FIFO (creates it, blocks until writers connect, removes it on exit)
synctell -i my-fifo

# Write a message into a FIFO (waits for a reader to appear, writes, then exits)
synctell -o my-fifo "hello, world"

# Write stdin into a FIFO
echo "hello" | synctell -o my-fifo

# Write with a timeout — exit 124 if no reader appears in 5 seconds
synctell -o my-fifo -t 5 "important message"

# Read with a timeout — exit 124 if no writer connects in 5 seconds
synctell -i my-fifo -t 5
```

## Examples

### Pipe between two shells

The reader (`-i`) creates the FIFO and stays alive, accepting one message
per writer connection. The writer (`-o`) waits for the FIFO to appear, opens
it, writes, and exits. Send SIGINT (Ctrl-C) or SIGTERM to the reader to
shut down cleanly — the FIFO is removed automatically.

**Shell 1** (reader — start this first, or concurrently with `-t` on the
writer):
```bash
synctell -i my-fifo
```

**Shell 2** (writer — the FIFO already exists, so this returns immediately):
```bash
synctell -o my-fifo "the answer is 42"
```

Shell 1 prints `the answer is 42`. The writer exits as soon as its message
is delivered. The reader stays alive, ready for more writers. Send it
SIGINT (Ctrl-C) or SIGTERM when you're done, and the FIFO is removed.

> **Note:** Without a timeout (`-t`), the writer exits immediately with
> code 1 if the FIFO is not yet present. If you're unsure which side
> starts first, give the writer a timeout:
>
> ```bash
> synctell -o my-fifo -t 10 "I'll wait up to 10 seconds"
> ```

### With a timeout

```bash
# Writer: exit 124 after 3 seconds if no reader is listening
synctell -o my-fifo -t 3 "are you there?"

# Reader: exit 124 after 3 seconds if no writer shows up.  Once one writer
# has connected, the reader stays alive indefinitely.
synctell -i my-fifo -t 3
```

### Multiple writers, one reader

A single reader accepts messages from any number of writers. Each message
arrives as a separate chunk on the reader's stdout. If a writer's data
doesn't end with a newline, the reader appends one — so writers that send
plain messages line up neatly on the receiver's output:

```bash
# Terminal 1: one reader, listening
synctell -i inbox.fifo

# Terminal 2, 3, 4: many writers, each delivering a message
synctell -o inbox.fifo "from agent-a"
synctell -o inbox.fifo "from agent-b"
synctell -o inbox.fifo "from agent-c"
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

### Chaining with other tools

Start the writer in the background — it waits for the FIFO to appear
(poll-once per second) and unblocks as soon as the reader connects:

```bash
# Writer: buffer stdin into a FIFO (waits for a reader, then delivers)
cat big-file.csv | synctell -o data-pipe &

# Reader: creates the FIFO, consumes stdin from each writer, streams to stdout
synctell -i data-pipe | sort | uniq -c > result.txt

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
synctell -i agent-b/inbox.fifo
```

```bash
# Agent A — writer (delivers a message; FIFO must already exist)
synctell -o agent-b/inbox.fifo "task complete: step 3 done"
```

The reader prints `task complete: step 3 done` and continues listening.

### Why this works so well for agents

**1. Blocking is synchronization.** When Agent A runs
`synctell -o inbox.fifo "message"`, it polls for the FIFO and **blocks**
until the FIFO appears (or its `-t` timeout expires). The writer cannot
deliver until the reader has created the FIFO, providing natural flow
control. No busy-waiting, no wasted CPU. The OS handles the rendezvous.
On the reader side, the signal-driven poll interval (1 second by default)
keeps overhead minimal.

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
messages, and removes the FIFO when it exits. Writers poll for the FIFO,
write, and exit. Agents can coordinate by agreeing on FIFO paths — the
path itself is the protocol.

**6. Timeout for liveness.** Use `-t` to avoid deadlocks. If the expected
peer never shows up, `synctell` exits with code 124 instead of hanging
forever. Your orchestration layer can detect this and re-route work.

### Example: multi-agent pipeline

Each step creates a one-shot reader for the next. The reader creates the
FIFO; the next step's writer polls for it:

```bash
# Step 1 → Step 2: Agent A reads input, Agent B consumes Agent A's output.
# (Some upstream producer writes the initial message via stdin or a writer.)
synctell -i pipeline/step2-input.fifo | process-image > /tmp/step2-output.bin &

# Step 2 → Step 3: Agent B's output is delivered to Agent C.
cat /tmp/step2-output.bin | synctell -o pipeline/step3-input.fifo &
synctell -i pipeline/step3-input.fifo | send-to-storage
wait
```

Each `-i` creates a FIFO, reads its input, removes the FIFO on exit.
Each `-o` polls for the FIFO, writes, exits. The polling writer blocks
until the upstream reader has created the FIFO, providing natural
backpressure between stages.

### Example: fan-in (many writers → one reader)

Many agents reporting into a single observer:

```bash
# Observer: one reader, accepts reports from any number of agents
synctell -i reports.fifo | tee -a /var/log/agents.log &
```

```bash
# Anywhere, any time: an agent drops a report into the observer
synctell -o reports.fifo "$(hostname): step done"
synctell -o reports.fifo "$(hostname): step done"
synctell -o reports.fifo "$(hostname): step done"
```

The observer's log grows line by line, with each report arriving as
soon as its writer connects. No need for a broker, a log-collector
daemon, or a network socket — just a FIFO.

## How It Works

`synctell` uses the `mkfifo(3)` system call to create POSIX named pipes
(in input mode only). The blocking open semantics of FIFOs (a write-open
blocks until a reader opens the other end, and vice versa) provide
natural synchronization without additional coordination.

In input mode (`-i`), `synctell` calls `mkfifo(3)` once at startup, then
loops reading messages: for each iteration it blocks on `open(path)` for
reading, which returns only once a writer has connected; it reads the
message to EOF, writes the bytes (plus a trailing newline if the writer's
data didn't already end with one) to stdout, and goes back to waiting
for the next writer. This continues until a SIGINT or SIGTERM sets a
shutdown flag that the read loop checks once per second.

In output mode (`-o`), `synctell` first buffers all of stdin (if no
positional message is given) into memory **before** opening the FIFO.
This prevents deadlock: if the FIFO were opened before reading stdin,
the pipe buffer could fill while blocked waiting for a reader. For very
large inputs, be aware that the entire input is held in memory.

When a timeout is specified (`-t`), output mode polls once per second
for the FIFO's existence. If the FIFO has not appeared by the deadline,
the process exits with code **124**. Input mode, on timeout without any
writer ever connecting, also removes the FIFO it created and exits with
code **124**. Once a reader has received at least one message, it stays
alive indefinitely — the timeout only governs the wait for the *first*
writer.

The reader removes the FIFO from the filesystem when it exits (whether
cleanly via SIGINT/SIGTERM or by timeout). Each `-i` invocation owns
its FIFO for its entire lifetime; writers come and go freely.

## Exit Codes

| Code | Meaning |
|------|---------|
| 0    | Success |
| 1    | General error (missing arguments, FIFO not present for writer, etc.) |
| 124  | Timeout — the expected peer did not appear within the specified duration |

## License

MIT