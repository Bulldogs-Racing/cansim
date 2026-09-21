# CanLab — End-to-End Arduino Firmware Simulation

## ROLE

Act as the lead engineer continuing the existing CanLab repository.

**Do not restart or redesign the project from scratch.**

First inspect the existing repository and preserve the working architecture and functionality already present. The repository already contains substantial CAN simulation, project management, WebSocket, frontend, analyzer, fault-injection, DBC, replay, sketch parsing, and STM32/Renode work.

The next objective is not to add more broad features.

The next objective is to make this workflow actually work:

```text
Launch CanLab
    ↓
Import/select an Arduino project
    ↓
Select Arduino board
    ↓
Compile the real Arduino firmware
    ↓
Load the compiled firmware into a real MCU emulator
    ↓
Connect the emulated MCU's CAN controller to CanLab's virtual CAN bus
    ↓
Run firmware
    ↓
Observe actual CAN TX/RX traffic live in CanLab
    ↓
Test the Arduino code without any physical CAN hardware
```

The simulator must execute the user's actual compiled firmware.

It must not replace the firmware with parsed CAN messages or a hand-written approximation.

---

# 1. CURRENT REPOSITORY REALITY

Treat the current repository as the starting point.

Already implemented:

- Rust CAN frame/model layer
- CAN identifiers
- virtual CAN buses
- arbitration
- deterministic simulation clock
- CAN timing
- CRC
- bit stuffing
- ACK/error behavior
- error counters and bus-off behavior
- deterministic faults
- YAML project format
- headless `canlab simulate`
- WebSocket server
- React/React Flow topology editor
- project editing and saving
- scripted CAN traffic
- CAN analyzer
- live event polling
- filtering/export/inspection
- bit-level frame inspection
- DBC decoding
- replay
- PCAP support
- Renode process supervision
- real STM32F103 firmware execution through Renode
- Arduino sketch static analysis
- `canlab sketch`

The current Arduino sketch importer is **not** a firmware execution system.

It currently:

```text
Arduino source
    ↓
static parser
    ↓
CAN send calls
    ↓
scripted MessageDecl
    ↓
logical CAN simulation
```

That is useful as a preview/tooling feature, but it is not sufficient for the new goal.

The desired execution path is:

```text
Arduino source
    ↓
Arduino build system
    ↓
ELF/HEX firmware
    ↓
AVR MCU emulator
    ↓
real Arduino firmware execution
    ↓
SPI
    ↓
MCP2515 model
    ↓
CAN controller interface
    ↓
CanLab virtual CAN bus
    ↓
CAN analyzer
```

---

# 2. PRIMARY PRODUCT GOAL

Make the following user experience real:

```text
1. Start CanLab.

2. Click:
   Import Arduino Project

3. Select an Arduino project directory.

4. CanLab detects:
   - .ino files
   - .cpp files
   - .h files
   - project structure
   - board/build configuration
   - required libraries

5. User selects:
   Arduino Uno

6. CanLab compiles the project.

7. CanLab shows:
   ✓ Compilation successful
   Firmware: ...
   Board: Arduino Uno
   Artifact: ...

8. User places:

   Arduino Uno
        │
        │
   virtual CAN bus
        │
   Arduino receiver

9. Press:
   ▶ Run

10. The actual firmware executes inside the MCU emulator.

11. The firmware's CAN controller communicates with
    the virtual CAN bus.

12. CAN Analyzer shows actual frames.

13. The user can change the Arduino firmware,
    recompile it, run it again, and observe
    different behavior.

No physical Arduino.

No physical CAN transceiver.

No physical CAN adapter.

No physical CAN endpoint.

```

This is the core milestone.

---

# 3. IMPORTANT DISTINCTION: PARSING VS EXECUTION

Keep the current Arduino sketch parser.

Do not delete it.

However, redefine its role.

The parser is:

```text
preflight / inspection / preview
```

not:

```text
firmware execution
```

It may be used to determine:

- probable CAN library
- CAN API usage
- source locations
- likely CAN IDs
- likely dependencies
- helpful diagnostics

But the parser must never be used as a substitute for actual firmware execution when the user selects **Run Firmware**.

For example, this firmware:

```cpp
byte counter = 0;

void loop() {
    engineData[0] = counter++;
    CAN.sendMsgBuf(CAN_ID, 0, 8, engineData);
}
```

must produce changing byte values during simulation.

A static parser cannot reproduce that correctly.

The end-to-end test must specifically verify this.

---

# 4. INITIAL HARDWARE TARGET

Do not attempt to support every Arduino board.

The first executable Arduino target is:

```text
Arduino Uno
ATmega328P
MCP2515
Classical CAN
```

Use this architecture:

```text
ATmega328P
     │
     │ SPI
     ▼
 MCP2515
     │
     ▼
 CAN interface
     │
     ▼
CanLab virtual CAN bus
```

The first supported Arduino library should be:

```text
mcp_can.h
```

The existing `mcp_can_sender.ino` fixture should become a real buildable firmware fixture instead of being parser-only.

Do not silently emulate the Arduino as an STM32.

Do not pretend the existing Renode STM32 backend is an Arduino backend.

---

# 5. AVR EXECUTION BACKEND

Create a genuine MCU execution backend for the Arduino Uno / ATmega328P.

The existing backend abstraction must be extended rather than bypassed.

Conceptually:

```rust
trait McuBackend {
    fn load_firmware(...);
    fn start(...);
    fn pause(...);
    fn resume(...);
    fn reset(...);
    fn stop(...);
    fn step(...);
    fn inspect(...);
}
```

Adapt the exact API to the existing architecture.

The AVR backend must execute actual compiled AVR instructions.

It must not:

- parse the source and simulate its intent
- detect `CAN.sendMsgBuf()` and inject a fake frame
- translate Arduino calls directly into CAN frames
- emulate an AVR by using the STM32 backend
- generate predetermined frame sequences

Actual firmware execution is required.

---

# 6. AVR EMULATOR SELECTION

Before implementing the backend, inspect available emulator technologies and choose an actual mechanism capable of executing ATmega328P firmware.

Prefer an existing emulator/library when practical.

The emulator must provide enough functionality for the first target:

```text
CPU
Flash
RAM
GPIO as needed
SPI
interrupts
timers/delay behavior as needed
UART where useful
```

Do not select an emulator merely because its name appears to support AVR.

Verify that it can actually execute an ATmega328P ELF/HEX image.

Document:

- why it was selected
- how firmware is loaded
- how SPI is exposed
- how interrupts are handled
- how simulated time is advanced

The emulator must be cleanly isolated behind the CanLab backend interface.

---

# 7. MCP2515 MODEL

Implement an actual MCP2515 peripheral model sufficient to run common Arduino `mcp_can` firmware.

The firmware must communicate with it through simulated SPI.

Architecture:

```text
Arduino firmware
       │
       ▼
ATmega328P SPI peripheral
       │
       ▼
MCP2515 registers
       │
       ▼
MCP2515 CAN controller
       │
       ▼
CanLab CAN controller interface
       │
       ▼
CanLab virtual CAN bus
```

Do not modify the Arduino firmware to call a special CanLab API.

The firmware should believe it is talking to an MCP2515 connected over SPI.

Initially support only the subset required by the existing fixtures and common `mcp_can` usage.

At minimum determine and test support for:

- reset
- read
- write
- bit modify
- RTS/TX operations used by the selected library
- CANCTRL
- CANSTAT
- CNF registers
- TX buffers
- RX buffers
- RX status
- interrupt flags
- CANINTF
- filters/masks as required
- normal mode
- receive behavior
- transmit completion
- SPI timing sufficient for realistic firmware behavior

Do not claim full MCP2515 compatibility until it is actually tested.

---

# 8. CONNECT MCP2515 TO THE EXISTING CAN ENGINE

The MCP2515 must not bypass the existing CAN engine.

It should interact through the existing CAN abstraction.

Use:

```text
MCP2515
    ↓
CanController abstraction
    ↓
CanBus
```

The existing CAN engine remains authoritative for:

- frame validity
- arbitration
- bus timing
- delivery
- faults
- errors
- timestamps
- event generation

The new MCU backend should feed real controller activity into that system.

Do not create a second independent CAN simulator inside the Arduino backend.

---

# 9. REAL-TIME / EVENT INTEGRATION

The current Renode path includes a batch-run workflow where firmware runs and its observed transmissions can later be imported into the analyzer.

That is useful infrastructure but is not enough for this product goal.

The new firmware execution path needs live event integration.

Desired architecture:

```text
MCU Emulator
     │
     │ controller events
     ▼
CanController
     │
     ▼
CanBus
     │
     ▼
Simulation Event Stream
     │
     ▼
WebSocket
     │
     ▼
Frontend
     │
     ▼
CAN Analyzer
```

The frontend must be able to display traffic while the simulation is running.

Do not require:

```text
run for 30 seconds
    ↓
wait for completion
    ↓
import trace
```

for the normal interactive firmware workflow.

A completed trace import may remain as a secondary feature.

---

# 10. FIRMWARE PROJECT IMPORT

Replace the current single-file-only Arduino import workflow with a proper Arduino project import flow.

The user must be able to select multiple project files.

Support:

```text
.ino
.cpp
.c
.h
```

and preserve the relative project structure.

The browser should be able to select a project directory, for example using directory selection where supported.

Conceptually:

```text
my-arduino-project/
├── my-arduino-project.ino
├── can_config.h
├── can_config.cpp
└── ...
```

The application should transmit the selected project files to the local CanLab backend.

Do not depend on the backend being able to magically access arbitrary files from the user's computer.

The imported workspace should have a controlled temporary server-side representation.

---

# 11. PROJECT WORKSPACE MODEL

Introduce a firmware-project abstraction.

Conceptually:

```text
FirmwareProject
├── name
├── files[]
├── board / FQBN
├── libraries[]
├── build status
├── artifact
└── diagnostics
```

Keep this separate from the existing CAN project.

A CanLab project may then reference:

```yaml
nodes:
    - id: arduino_sender
      device: arduino_uno
      backend: avr
      firmware_project: ./firmware/arduino_sender
      can:
          bus: vehicle_bus
```

You may evolve the existing schema version if necessary.

Provide migration behavior for existing version-1 projects.

Do not break existing virtual/STM32 projects.

---

# 12. ARDUINO COMPILATION

The application needs a real Arduino compilation service.

Prefer using the standard Arduino build tooling rather than implementing the entire Arduino build system yourself.

The initial target should support something equivalent to:

```text
arduino-cli compile
```

for:

```text
Arduino Uno
arduino:avr:uno
```

The exact implementation may differ if a better local build mechanism is justified.

The compilation system must:

- locate the configured toolchain
- detect missing tools
- compile the complete project
- compile dependencies/libraries
- produce an ELF artifact
- produce HEX when appropriate
- capture compiler stdout/stderr
- expose diagnostics to the GUI
- return structured success/failure information

Do not treat compilation as optional for the executable workflow.

---

# 13. TOOL DISCOVERY

Extend `canlab doctor`.

It should report things such as:

```text
CanLab Doctor

Rust                     ✓
Node.js                  ✓
Frontend dependencies    ✓

Arduino CLI              ✓
AVR toolchain            ✓
ATmega328P backend       ✓
MCP2515 backend           ✓

Renode                   ✓ / optional
STM32 toolchain           ✓ / optional
```

Missing Arduino dependencies should produce actionable errors.

Example:

```text
Arduino compilation is unavailable.

Missing:
  arduino-cli

The selected target requires:
  Arduino Uno
  FQBN: arduino:avr:uno

Install/configure Arduino CLI or select another supported backend.
```

Do not silently fall back to static parsing.

---

# 14. COMPILATION UI

Add a proper firmware-project panel.

It should provide:

```text
Arduino Firmware

Project:
[ my-can-project ]

Board:
[ Arduino Uno ]

Build:
[ Compile ]

Status:
● Ready
```

After compiling:

```text
✓ Build successful

Board:
Arduino Uno

Artifact:
build/my-can-project.elf

[View Build Log]
```

On failure:

```text
✗ Build failed

src/main.cpp:47:
error: ...
```

The user must be able to understand why the firmware cannot run.

---

# 15. MCU NODE UI

Clicking the Arduino node should show something like:

```text
Arduino Uno

Firmware
my-can-project

Backend
AVR

CAN controller
MCP2515

CAN bus
vehicle_bus

Build
✓ compiled

Simulation
● stopped

[Compile]
[Run]
[Pause]
[Reset]
[Stop]
```

Do not expose Renode/AVR internals in the beginner UI unless useful.

The frontend should consume backend capabilities rather than checking:

```text
if device == arduino_uno
```

throughout the UI.

---

# 16. CAN BUS VISUALIZATION

Reuse the existing React Flow topology editor.

The user should be able to build:

```text
┌────────────────┐
│ Arduino Sender │
└───────┬────────┘
        │
        │
════════╪══════════
     CAN BUS
════════╪══════════
        │
        │
┌───────┴────────┐
│ Arduino RX     │
└────────────────┘
```

The existing topology editor should remain intact.

Add runtime indicators:

```text
Arduino Sender
    ● TX

CAN BUS
    ● traffic

Arduino Receiver
    ● RX
```

Reuse the existing analyzer/event stream rather than creating a parallel traffic system.

---

# 17. REAL FIRMWARE TEST

Create two actual Arduino firmware fixtures.

### Sender

Use the existing `mcp_can_sender.ino` concept.

It should send:

```text
ID 0x100
DLC 8
```

and dynamically change at least one byte:

```cpp
engineData[0] = counter++;
```

### Receiver

Create a real Arduino Uno + MCP2515 receiving firmware.

It should receive frames and expose observable state through something such as:

- UART
- firmware log
- receiver counter
- received CAN frame

The receiver must interact with the MCP2515 through SPI.

Do not make the receiver a fake virtual node.

---

# 18. DEFINITIVE END-TO-END ACCEPTANCE TEST

This is the most important test in the project.

The test must prove:

```text
Arduino source
      ↓
real compilation
      ↓
real AVR firmware execution
      ↓
real SPI interaction
      ↓
real MCP2515 model
      ↓
virtual CAN bus
      ↓
second real emulated node
      ↓
CAN Analyzer
```

Procedure:

1. Import the sender Arduino project.
2. Import the receiver Arduino project.
3. Select Arduino Uno for both.
4. Select MCP2515 CAN controller.
5. Attach both to the same virtual CAN bus.
6. Compile both projects.
7. Start simulation.
8. Run both firmware images.
9. Observe sender TX frames.
10. Observe receiver RX frames.
11. Verify ID `0x100`.
12. Verify DLC.
13. Verify payload.
14. Verify timestamps.
15. Verify the dynamically incrementing counter byte changes across transmissions.

The test must fail if the implementation is merely reading the source and generating scripted messages.

---

# 19. SECOND ACCEPTANCE TEST: ARBITRATION

Create two real firmware nodes that attempt to transmit at the same simulation instant:

```text
Arduino A → 0x300
Arduino B → 0x100
```

The existing CAN core should determine:

```text
0x100 wins arbitration
0x300 loses arbitration
```

The firmware should experience the result through its simulated CAN controller.

The backend must not simply report a precomputed scripted winner.

---

# 20. THIRD ACCEPTANCE TEST: RUNTIME DEPENDENCY

The Arduino firmware must demonstrate behavior that cannot be determined through static parsing.

Example:

```cpp
counter++;
```

The analyzer should eventually show:

```text
0x100 ... 00 ...
0x100 ... 01 ...
0x100 ... 02 ...
0x100 ... 03 ...
```

or an equivalent observable runtime progression.

This is a required proof that firmware is actually executing.

---

# 21. PRESERVE SCRIPTED TRAFFIC

Do not remove the current scripted-message feature.

It is still useful for:

- protocol tests
- quick CAN experimentation
- regression tests
- users who do not have firmware
- deterministic traffic generation
- arbitration demonstrations
- fault testing

Clearly distinguish:

```text
Scripted simulation
```

from:

```text
Firmware simulation
```

The UI should make the distinction obvious.

For example:

```text
Execution mode:
● Firmware
○ Scripted CAN
```

---

# 22. PRESERVE STATIC SKETCH IMPORT

The existing sketch parser remains useful.

Keep:

```bash
canlab sketch ...
```

It should remain available for:

- source inspection
- library detection
- preview
- diagnostics
- future dependency discovery

But change the GUI wording so users do not mistake it for firmware execution.

Do not display:

```text
Sketch import
```

as though it loads executable firmware when it only extracts CAN intent.

---

# 23. BACKEND CAPABILITY MODEL

Introduce backend capability reporting.

For example:

```text
Device: arduino_uno

Backend:
  AVR emulator

Capabilities:
  ✓ firmware execution
  ✓ SPI
  ✓ MCP2515
  ✓ Classical CAN
  ✓ UART
  ✗ CAN FD
```

The UI should ask the backend what it supports.

Do not hard-code capability assumptions throughout React.

---

# 24. ERROR HANDLING

Every unsupported path must fail explicitly.

Examples:

```text
Arduino Uno firmware execution is unavailable because
the AVR backend is not installed.
```

```text
The selected sketch requires library MCP_CAN,
but the library is unavailable to the compiler.
```

```text
The selected board is recognized but no emulator
backend exists for it yet.
```

```text
MCP2515 initialization used an unsupported register
operation at SPI command 0xXX.
```

Never silently turn unsupported firmware into:

```text
virtual scripted traffic
```

That would hide failures and make simulation results misleading.

---

# 25. PHYSICAL HARDWARE MUST NOT BE REQUIRED

The first implementation must operate entirely in software.

No:

- Arduino USB connection
- CAN adapter
- real CAN transceiver
- SocketCAN
- vcan interface
- physical endpoint

should be required for the core workflow.

The topology should be:

```text
          SOFTWARE ONLY

 Arduino Emulator
       │
       ▼
    MCP2515
       │
       ▼
 Virtual CAN Bus
       │
       ▼
 Arduino Emulator
       │
       ▼
 CAN Analyzer
```

SocketCAN and real CAN hardware remain future features.

---

# 26. SECURITY

Firmware and uploaded project files are untrusted inputs.

Do not execute arbitrary project-provided shell scripts.

Do not execute:

```text
./build.sh
make whatever
custom_project_command
```

merely because a project contains such a file.

Compilation should happen through a controlled configured toolchain interface.

Use temporary workspaces.

Prevent:

- path traversal
- writing arbitrary files outside the workspace
- arbitrary command execution through project metadata
- unbounded file sizes
- unbounded compiler output

Clean up temporary firmware workspaces.

---

# 27. PERFORMANCE

Do not make the frontend dictate emulator timing.

The model should remain:

```text
Firmware/Emulator
      ↓
Simulation events
      ↓
WebSocket
      ↓
Frontend rendering
```

The UI may throttle rendering.

The simulation must not be throttled to the browser's frame rate.

The analyzer should continue using bounded capture storage/ring-buffer behavior.

---

# 28. EXISTING RENODE WORK

Do not delete the existing Renode implementation.

Keep:

```text
STM32F103
    ↓
Renode
    ↓
CAN
```

working.

Refactor where necessary so Arduino and STM32 share the same backend abstraction.

The architecture should become:

```text
                McuBackend
                  │
          ┌───────┴────────┐
          │                │
        Renode         AVR backend
          │                │
       STM32F103       ATmega328P
          │                │
       bxCAN           SPI/MCP2515
          │                │
          └──────┬─────────┘
                 │
              CanBus
```

Do not make the CAN core know which emulator is running.

---

# 29. PROJECT ARCHITECTURE

Maintain the existing layering:

```text
Frontend
    ↓
Application/WebSocket API
    ↓
Firmware/project management
    ↓
MCU execution backend
    ↓
Controller/peripheral model
    ↓
CAN core
    ↓
Future physical CAN layer
```

Do not collapse:

```text
MCU
CAN controller
CAN bus
physical transceiver
```

into one object.

---

# 30. IMPLEMENTATION ORDER

Implement this incrementally.

## Step 1 — Baseline

Inspect and run the existing test suite.

Verify:

```bash
cargo test
```

and:

```bash
cd frontend
npm install
npm run typecheck
npm run build
```

Fix baseline build/environment issues before changing architecture.

Do not assume the repository is healthy without testing it.

---

## Step 2 — Firmware workspace import

Implement:

```text
Arduino project directory
        ↓
Frontend file selection
        ↓
Backend workspace
        ↓
safe project representation
```

Test multiple `.ino/.cpp/.h` files.

---

## Step 3 — Arduino toolchain service

Implement:

```text
FirmwareProject
    ↓
BuildService
    ↓
Arduino CLI/toolchain
    ↓
ELF/HEX
```

Add unit/integration tests for:

- successful compile
- missing compiler
- invalid source
- missing library
- unsupported board

---

## Step 4 — AVR emulator proof

Before building the whole UI, prove:

```text
ATmega328P ELF
       ↓
AVR emulator
       ↓
CPU executes instructions
       ↓
observable firmware behavior
```

Create the smallest firmware fixture possible.

---

## Step 5 — SPI proof

Prove:

```text
ATmega328P firmware
       ↓
SPI write
       ↓
MCP2515 model
```

with register-level tests.

---

## Step 6 — MCP2515 TX

Prove:

```text
Arduino firmware
       ↓
mcp_can library
       ↓
SPI
       ↓
MCP2515
       ↓
CanBus
       ↓
CAN frame
```

No physical hardware.

No scripted-message substitution.

---

## Step 7 — MCP2515 RX

Add actual receive behavior:

```text
CanBus
   ↓
MCP2515 RX buffer
   ↓
SPI
   ↓
Arduino firmware
```

---

## Step 8 — Two-node execution

Run two actual Arduino firmware images against one virtual CAN bus.

Test:

```text
Arduino TX
    ↓
CAN BUS
    ↓
Arduino RX
```

---

## Step 9 — Live event stream

Expose actual emulator/controller events through the existing WebSocket event system.

Do not wait for job completion before displaying traffic.

---

## Step 10 — GUI firmware workflow

Implement:

```text
Import Project
    ↓
Select Board
    ↓
Compile
    ↓
Create/attach firmware node
    ↓
Run
    ↓
Live analyzer
```

---

## Step 11 — End-to-end acceptance

Run the exact acceptance tests in this prompt.

Only call the milestone complete after they pass.

---

# 31. DO NOT IMPLEMENT YET

Do not distract the implementation with:

- Teensy execution
- every Arduino board
- CAN FD
- physical CAN voltage simulation
- SPICE
- PCB simulation
- SocketCAN
- USB CAN adapters
- cloud compilation
- cloud collaboration
- graphical DBC editor
- UDS
- AUTOSAR
- J1939
- automotive Ethernet

The current milestone is:

```text
Arduino Uno
+
ATmega328P
+
MCP2515
+
real compiled firmware
+
virtual CAN bus
+
live CAN analyzer
```

---

# 32. DEFINITION OF DONE FOR THIS MILESTONE

The milestone is complete only when a new user can do this:

```text
Open CanLab
      ↓
Import Arduino project
      ↓
Select Arduino Uno
      ↓
Compile
      ↓
Build succeeds
      ↓
Place Arduino node
      ↓
Attach to virtual CAN bus
      ↓
Press Run
      ↓
Actual firmware executes
      ↓
MCP2515 communicates over simulated SPI
      ↓
CAN frame reaches virtual CAN bus
      ↓
CAN Analyzer shows the frame
```

And, critically:

```text
No physical Arduino.
No physical CAN adapter.
No physical CAN endpoint.
```

The dynamic behavior must also prove that the firmware was actually executed.

For the existing sender fixture:

```cpp
engineData[0] = counter++;
```

the simulator must observe changing payload bytes over successive transmissions.

A solution that produces the expected CAN frames by parsing the source is **not acceptable**.

A solution that replaces Arduino firmware with `MessageDecl` rows is **not acceptable**.

A solution that silently runs Arduino firmware as an STM32 is **not acceptable**.

A solution that runs firmware after the simulation and merely imports a finished trace is **not the desired interactive execution model**.

The goal is:

> **Compile and execute real Arduino firmware against a real virtual CAN network inside CanLab, so the user can develop and test CAN firmware without a single piece of physical CAN hardware.**

That is the next major product milestone.

---

# 33. ENGINEERING RULE

Do not optimize for adding more UI features.

Optimize for proving this path:

```text
REAL SOURCE
    ↓
REAL COMPILER
    ↓
REAL MCU INSTRUCTIONS
    ↓
REAL PERIPHERAL INTERACTION
    ↓
REAL CAN CONTROLLER MODEL
    ↓
CORRECT VIRTUAL CAN BUS
    ↓
OBSERVABLE LIVE TRAFFIC
```

Once that path works, the existing visual editor and analyzer become the interface to a genuinely useful firmware simulator rather than a visual wrapper around scripted CAN traffic.

Preserve the existing CAN core.

Preserve the existing STM32/Renode backend.

Preserve the static sketch parser.

Add the missing execution path cleanly.

Do not rewrite working subsystems merely to make the new feature fit.
