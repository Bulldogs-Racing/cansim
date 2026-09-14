# CanLab — Open-Source Visual CAN Network & MCU Simulator

## ROLE

You are the lead software architect and engineer responsible for designing and implementing **CanLab**, an open-source CAN-network simulation and embedded-firmware development environment.

Do not treat this as a generic electronics simulator.

The primary purpose of CanLab is:

> **Allow users to visually construct a network containing simulated Arduino, STM32, and Teensy microcontrollers, run real compiled firmware on those simulated devices, connect their CAN peripherals/controllers to a simulated CAN bus, inspect and debug CAN traffic, inject CAN/network faults, and eventually bridge simulated CAN networks to real CAN hardware.**

The project should be architected so that the CAN simulation engine, MCU emulation backends, project format, frontend, and hardware interfaces are independent components.

The application should eventually be capable of:

```text
                 CanLab

       ┌─────────────────────────┐
       │      Visual Editor      │
       │                         │
       │ STM32 ──────┐           │
       │             │           │
       │ Arduino ────┼── CAN ────┼── Teensy
       │             │           │
       └─────────────┼───────────┘
                     │
                     ▼
              Simulation Engine
                     │
             ┌───────┴────────┐
             ▼                ▼
          Renode          Future backends
             │
             ▼
        MCU emulation
             │
             ▼
          CAN bus
             │
             ▼
       CAN Analyzer
```

---

# 1. CORE PRODUCT PRINCIPLES

Follow these principles throughout the implementation.

## 1.1 CAN-first

CanLab is NOT initially a general SPICE/electronics simulator.

Do NOT spend early development time implementing:

* analog circuits
* arbitrary resistors
* capacitors
* inductors
* op-amps
* transistors
* arbitrary PCB simulation
* full electrical engineering simulation

The initial component system should focus on:

* microcontrollers
* CAN controllers
* CAN transceivers
* CAN buses
* digital I/O where useful
* terminals
* logic analyzers
* CAN analyzers
* fault-injection devices

The architecture may eventually permit general circuit simulation, but that must not compromise the CAN-focused MVP.

---

# 2. PRIMARY GOALS

CanLab must eventually support:

### Microcontrollers

Initial targets:

1. STM32F103
2. Arduino Uno / ATmega328P
3. Arduino + MCP2515
4. Teensy 4.1 / NXP i.MX RT1062

Future targets should be easy to add.

### CAN

Support:

* Classical CAN
* Standard 11-bit identifiers
* Extended 29-bit identifiers
* RTR frames
* DLC
* 8-byte Classical CAN payloads
* arbitration
* dominant/recessive logic
* ACK
* CRC
* bit stuffing
* error handling
* error counters
* error passive
* bus-off
* CAN FD architecture
* CAN FD eventually
* bit-rate switching eventually

### Firmware

Allow real compiled firmware to execute inside the simulated MCU backend.

Examples:

```text
STM32 firmware ELF
        ↓
simulated STM32
        ↓
simulated bxCAN
        ↓
CAN bus
```

and:

```text
Arduino firmware
        ↓
simulated ATmega328P
        ↓
SPI
        ↓
simulated MCP2515
        ↓
CAN bus
```

and eventually:

```text
Teensy firmware
        ↓
simulated i.MX RT1062
        ↓
FlexCAN
        ↓
CAN bus
```

---

# 3. ARCHITECTURE

Use a layered architecture.

```text
┌───────────────────────────────────────────────┐
│                 Frontend                      │
│ React / TypeScript                            │
├───────────────────────────────────────────────┤
│              Application API                  │
│ Project management / simulation control       │
├───────────────────────────────────────────────┤
│            Simulation Manager                 │
├───────────────────────────────────────────────┤
│                CAN Core                       │
│ bus / arbitration / frames / timing / errors  │
├───────────────────────────────────────────────┤
│           MCU Backend Interface               │
├───────────────────────────────────────────────┤
│                  Renode                       │
│          Initial emulation backend            │
├───────────────────────────────────────────────┤
│             Future backends                   │
│ QEMU / native / other emulators               │
├───────────────────────────────────────────────┤
│             Hardware Interfaces               │
│ SocketCAN / USB-CAN                           │
└───────────────────────────────────────────────┘
```

Never tightly couple the frontend to Renode.

Never make Renode-specific assumptions part of the core CAN model.

---

# 4. RECOMMENDED TECHNOLOGY

Use:

## Backend

Rust.

Reasons:

* strong type safety
* good concurrency model
* suitable for simulation engines
* good binary/tooling ecosystem
* easy creation of a standalone CLI
* good future library reuse

## Frontend

TypeScript + React.

Use a graph/canvas library such as React Flow if it materially simplifies the editor.

Use SVG/canvas/WebGL only where appropriate.

## Communication

Use a local WebSocket API between frontend and backend.

The architecture should also permit a future HTTP/REST API where useful.

---

# 5. REPOSITORY STRUCTURE

Use a structure approximately like:

```text
canlab/
│
├── README.md
├── LICENSE
├── CONTRIBUTING.md
├── SECURITY.md
├── CODE_OF_CONDUCT.md
├── Cargo.toml
├── package.json
│
├── core/
│   ├── can/
│   │   ├── frame.rs
│   │   ├── frame_id.rs
│   │   ├── bus.rs
│   │   ├── node.rs
│   │   ├── arbitration.rs
│   │   ├── timing.rs
│   │   ├── crc.rs
│   │   ├── stuffing.rs
│   │   ├── errors.rs
│   │   ├── state.rs
│   │   └── mod.rs
│   │
│   ├── simulation/
│   │   ├── engine.rs
│   │   ├── scheduler.rs
│   │   ├── event.rs
│   │   └── clock.rs
│   │
│   ├── devices/
│   │   ├── device.rs
│   │   ├── can_controller.rs
│   │   ├── transceiver.rs
│   │   ├── mcp2515.rs
│   │   └── mod.rs
│   │
│   └── project/
│       ├── project.rs
│       ├── component.rs
│       ├── connection.rs
│       └── serialization.rs
│
├── backends/
│   ├── backend-api/
│   ├── renode/
│   │   ├── manager.rs
│   │   ├── machine.rs
│   │   ├── can.rs
│   │   └── templates/
│   │
│   └── future/
│
├── hardware/
│   └── socketcan/
│
├── server/
│   ├── api/
│   ├── websocket/
│   └── main.rs
│
├── cli/
│   └── main.rs
│
├── frontend/
│   ├── src/
│   │   ├── editor/
│   │   ├── components/
│   │   ├── simulation/
│   │   ├── analyzer/
│   │   ├── waveform/
│   │   ├── project/
│   │   ├── settings/
│   │   └── app/
│   │
│   └── package.json
│
├── firmware/
│   ├── examples/
│   │   ├── stm32/
│   │   └── arduino/
│   │
│   └── tests/
│
├── tests/
│   ├── can/
│   ├── integration/
│   └── firmware/
│
└── docs/
    ├── architecture/
    ├── can/
    ├── devices/
    ├── firmware/
    └── user-guide/
```

Adjust the exact structure if a better architecture is discovered, but preserve the separation of concerns.

---

# 6. CAN CORE

Implement the CAN protocol engine independently of any MCU emulator.

The CAN core must be usable without the frontend.

## Frame

Create a strongly typed frame representation.

Conceptually:

```rust
struct CanFrame {
    id: CanId,
    frame_type: FrameType,
    dlc: u8,
    data: Vec<u8>,
}
```

Represent:

* standard ID
* extended ID
* data frame
* remote frame
* Classical CAN
* future CAN FD

Validate:

* identifier ranges
* DLC
* payload size
* frame type compatibility

Do not allow invalid states where practical.

---

# 7. CAN BUS

Implement a virtual CAN bus abstraction.

Example:

```text
CanBus
 ├── Node A
 ├── Node B
 ├── Node C
 └── Node D
```

The bus must:

* register nodes
* unregister nodes
* accept transmissions
* perform arbitration
* deliver winning frames
* report collisions/arbitration losses
* manage timing
* expose events to observers
* support fault injection
* support bus state

---

# 8. ARBITRATION

Implement real CAN arbitration semantics.

Example:

```text
Node A: 0x300
Node B: 0x100
Node C: 0x200
```

The lower identifier must win.

The simulator must expose arbitration events.

For example:

```text
Arbitration started

Node A: 0x300
Node B: 0x100
Node C: 0x200

Winner: Node B
Lost:
  Node A
  Node C
```

Eventually the UI should visualize this.

---

# 9. TIMING

Do not make the entire simulator dependent on wall-clock timing.

Create a simulation clock.

Support:

* simulated timestamps
* deterministic simulation
* accelerated simulation
* paused simulation
* step-by-step simulation
* real-time mode where practical

The engine should be deterministic when running the same project with the same seed/configuration.

Represent time using a precise unit such as nanoseconds or another appropriate integer-based representation.

Avoid floating-point time for core scheduling.

---

# 10. CAN BIT MODEL

Eventually support a bit-level representation.

Represent:

* SOF
* arbitration
* control
* data
* CRC
* ACK
* EOF
* inter-frame spacing

Implement bit stuffing.

The bit-level representation must be separate from the higher-level frame API.

The high-level frame API should remain convenient.

---

# 11. CRC

Implement the appropriate Classical CAN CRC behavior.

Provide unit tests using known-good frame examples.

Do not invent protocol behavior.

Where protocol details are uncertain, consult authoritative CAN specifications/references before implementation.

Document protocol assumptions.

---

# 12. ERROR HANDLING

Model:

* bit errors
* stuff errors
* CRC errors
* form errors
* ACK errors
* error frames
* transmit error counter
* receive error counter
* error active
* error passive
* bus-off

The simulator should expose:

```text
TEC
REC
State
```

for each CAN controller.

Example:

```text
Node: ECU

TEC: 128
REC: 4

State:
ERROR PASSIVE
```

---

# 13. CAN CONTROLLER ABSTRACTION

Do NOT make STM32, Arduino, and Teensy directly communicate with the bus.

Introduce:

```text
MCU
 ↓
CAN peripheral/controller
 ↓
CAN controller interface
 ↓
transceiver
 ↓
CAN bus
```

Define a common interface.

Conceptually:

```rust
trait CanController {
    fn transmit(...);
    fn receive(...);
    fn configure(...);
    fn reset(...);
    fn status(...);
}
```

The exact API may differ.

---

# 14. MCP2515

Implement an MCP2515 model sufficient for real Arduino CAN libraries.

Architecture:

```text
ATmega328P
    │
    │ SPI
    ▼
MCP2515
    │
    ▼
CAN transceiver
    │
    ▼
CAN bus
```

Support relevant:

* SPI commands
* registers
* configuration
* bit timing
* TX buffers
* RX buffers
* interrupts
* masks
* filters
* error states

Prioritize compatibility with common Arduino MCP2515 libraries.

Do not fake the peripheral at the firmware API level.

The firmware should interact with the simulated MCP2515 through SPI exactly as it would on hardware.

---

# 15. STM32F103

Initial STM32 target:

**STM32F103C8 / closely compatible STM32F103 variant.**

The objective is to run real firmware.

Support the MCU's relevant:

* CPU
* memory
* clocks as required
* GPIO where needed
* CAN peripheral
* interrupts
* relevant timers/peripherals needed by common CAN firmware

Do not implement the entire STM32 peripheral set unless required.

Prioritize CAN functionality.

Support firmware supplied as ELF/bin/hex as appropriate for the backend.

---

# 16. TEENSY 4.1

Add after STM32 and Arduino are working.

Target:

**Teensy 4.1 / NXP i.MX RT1062**

The relevant CAN peripheral is FlexCAN.

Architecture:

```text
i.MX RT1062
     │
   FlexCAN
     │
CAN transceiver
     │
  CAN bus
```

Do not pretend Teensy has the same peripheral implementation as STM32.

The common CAN interface must hide those hardware-specific differences.

---

# 17. MCU BACKEND INTERFACE

Create a backend abstraction.

Conceptually:

```text
McuBackend
 ├── load_firmware()
 ├── start()
 ├── stop()
 ├── reset()
 ├── pause()
 ├── resume()
 ├── step()
 ├── inspect()
 └── peripherals()
```

Renode is the first implementation.

Future implementations may include:

* QEMU
* native MCU emulation
* another emulator
* remote simulation

The frontend must not know which backend is active.

---

# 18. RENODE INTEGRATION

Use Renode as the initial MCU execution backend.

Do not fork Renode unnecessarily.

Treat Renode as an external backend/dependency.

Create a manager capable of:

1. starting Renode
2. creating machines
3. loading platform descriptions
4. loading firmware
5. connecting peripherals
6. connecting CAN
7. starting/stopping simulation
8. collecting CAN events
9. exposing debug state
10. shutting down cleanly

Use generated Renode scripts/configurations where appropriate.

Do not hard-code temporary paths.

Make Renode installation/configuration discoverable and configurable.

---

# 19. RENODE PROCESS MANAGEMENT

Handle:

* startup failure
* missing Renode installation
* crashed Renode process
* timeout
* malformed project
* invalid firmware
* backend incompatibility
* clean shutdown
* restart

Never leave orphaned simulation processes.

Provide useful errors to the UI.

Example:

```text
Renode could not be started.

Possible causes:
- Renode is not installed
- configured path is invalid
- platform configuration failed

[Configure Renode]
[View Logs]
```

---

# 20. PROJECT FORMAT

Create a human-readable project format.

Prefer YAML or JSON initially.

Example:

```yaml
version: 1

simulation:
  mode: deterministic

buses:
  - id: vehicle_bus
    type: can
    bitrate: 500000
    fd: false

nodes:

  - id: engine_ecu
    device: stm32f103
    backend: renode
    firmware: ./firmware/engine.elf
    can:
      bus: vehicle_bus

  - id: dashboard
    device: arduino_uno
    backend: renode
    firmware: ./firmware/dashboard.hex
    peripherals:
      - type: mcp2515
        spi: spi0
    can:
      bus: vehicle_bus
```

The format must be versioned.

Design for backwards compatibility.

---

# 21. PROJECT FILE SAFETY

Never blindly execute arbitrary commands from project files.

Treat project files as untrusted input.

Do not permit project configuration to execute arbitrary shell commands unless explicitly and safely designed.

Validate:

* paths
* component types
* firmware paths
* backend identifiers
* numeric values
* network configuration

Prevent path traversal where applicable.

---

# 22. FRONTEND

Create a visual editor.

The editor is a **network topology editor**, not a full SPICE schematic editor.

Users should be able to drag:

```text
STM32
Arduino
Teensy
MCP2515
CAN Bus
CAN Analyzer
CAN Transceiver
```

onto a canvas.

---

# 23. EDITOR

Support:

* pan
* zoom
* select
* multi-select
* move
* delete
* duplicate
* connect
* disconnect
* snap-to-grid
* undo
* redo
* keyboard shortcuts
* save
* load

Connections should be visually obvious.

---

# 24. CAN BUS VISUALIZATION

Represent a CAN bus as an actual network object.

Do not require users to draw every physical wire in the MVP.

For example:

```text
       ┌─────────┐
       │ STM32   │
       └────┬────┘
            │
            │
      ══════╪══════ CAN BUS
            │
       ┌────┴────┐
       │ Arduino │
       └─────────┘
```

Users can later choose a more detailed physical representation.

---

# 25. COMPONENT LIBRARY

Initial components:

### Microcontrollers

* STM32F103
* Arduino Uno
* Teensy 4.1

### CAN

* CAN Bus
* MCP2515
* Generic CAN transceiver
* SN65HVD230
* TJA1050

### Debugging

* CAN Analyzer
* Logic Analyzer
* Serial Terminal

### Digital

Eventually:

* LED
* button
* switch
* potentiometer

These are secondary.

---

# 26. MCU CONFIGURATION UI

Clicking an MCU should show:

```text
STM32F103

Firmware
[ engine.elf ] [Browse]

Backend
[ Renode ]

CAN Interface
[ CAN1 ]

CAN Bus
[ vehicle_bus ]

Status
● Running

[Start]
[Pause]
[Reset]
[Stop]
```

Expose useful hardware/debug information without overwhelming the user.

---

# 27. SIMULATION CONTROLS

Global controls:

```text
▶ Run
⏸ Pause
⏹ Stop
↻ Reset
⏭ Step
```

Provide:

* run continuously
* pause
* reset
* step
* simulation speed
* real-time/deterministic mode where supported

---

# 28. CAN ANALYZER

The CAN analyzer is a core product feature.

Display:

```text
TIME       NODE       ID       DLC       DATA

0.001ms    ECU        0x100    8         01 02 03 04 05 06 07 08
0.102ms    BMS        0x180    8         FF 00 00 01 00 00 00 00
0.205ms    DASH       0x200    4         20 01 00 00
```

Support:

* sorting
* filtering
* pause capture
* clear
* export
* search
* frame selection
* TX/RX indication
* sender
* timestamp

---

# 29. FRAME INSPECTOR

Clicking a frame should show:

```text
CAN FRAME

ID
0x180

Decimal
384

Format
Standard

DLC
8

Data
FF 00 00 01 00 00 00 00

Sender
BMS

Timestamp
102.4 μs

Bitrate
500 kbit/s
```

Eventually support bit-level decoding.

---

# 30. LIVE TRAFFIC INDICATORS

The canvas should optionally indicate CAN traffic.

Example:

```text
STM32
   │
   ├──── TX ●
   │
CAN BUS
   │
   └──── RX ●
```

Use subtle animation.

Do not make the UI visually noisy.

Allow traffic animation to be disabled.

---

# 31. CAN FILTERS

Provide filters:

```text
ID:
[0x100]

Mask:
[0x7FF]

Node:
[All]

Direction:
[TX/RX]

Frame:
[All]
```

Support:

* exact ID
* range
* mask
* sender
* receiver
* frame type

---

# 32. LOGGING

Provide structured logs.

Categories:

* simulation
* MCU
* CAN
* backend
* frontend
* hardware

Allow users to inspect logs.

Don't rely exclusively on stdout.

---

# 33. WAVEFORM VIEW

Eventually implement a bit-level CAN waveform viewer.

Show:

```text
CANH
───────┐     ┌────────
       └─────┘

CANL
───────┘     └────────
       ┌─────┐
```

Support:

* zoom
* pan
* cursor
* timestamp
* bit boundaries
* decoded fields
* dominant/recessive state

Overlay:

```text
SOF
ARBITRATION
CONTROL
DATA
CRC
ACK
EOF
```

---

# 34. ARBITRATION VISUALIZATION

When multiple nodes transmit simultaneously, optionally show:

```text
Arbitration

ECU       0x300  ──┐
BMS       0x100  ──┼──► BMS wins
Dashboard 0x200  ──┘

Winning ID: 0x100
```

This should be educational as well as diagnostic.

---

# 35. FAULT INJECTION

Add a fault-injection system.

Initial UI:

```text
FAULT INJECTION

Frame faults

[ ] Drop frame
[ ] Corrupt data
[ ] CRC error
[ ] ACK error

Bus faults

[ ] Disconnect CANH
[ ] Disconnect CANL
[ ] Force dominant
[ ] Force recessive

Node faults

[ ] Force bus-off
[ ] Disable node
[ ] Add latency
```

Provide configurable:

* probability
* duration
* affected node
* affected frame
* affected bus

---

# 36. DETERMINISTIC FAULTS

Faults should be reproducible.

For example:

```yaml
faults:
  - type: drop_frame
    node: bms
    id: 0x180
    probability: 0.1
    seed: 12345
```

This enables repeatable debugging.

---

# 37. CAN FD

Do not implement CAN FD in the MVP.

However, design the frame and bus APIs so CAN FD can be added without rewriting everything.

Eventually support:

* up to 64-byte payload
* FDF
* BRS
* ESI
* separate arbitration/data bit rates
* CAN FD CRC behavior

---

# 38. DBC SUPPORT

After CAN FD/basic stability, implement DBC support.

Allow importing:

```text
vehicle.dbc
```

Decode frames.

Example:

```text
0x180 BMS_STATUS

Battery SOC
82.4 %

Battery Voltage
397.2 V

Battery Current
-31.2 A

Temperature
29 °C
```

Provide a DBC editor later if practical.

---

# 39. SOCKETCAN

Eventually support Linux SocketCAN.

Architecture:

```text
CanLab
   │
   ▼
SocketCAN
   │
   ▼
vcan0 / can0
   │
   ▼
Linux CAN ecosystem
```

Allow:

```bash
candump
cansend
```

and other CAN utilities to interact with the simulated network where appropriate.

Do not make Linux SocketCAN a requirement for the core simulator.

---

# 40. REAL HARDWARE BRIDGE

Eventually support:

```text
SIMULATED NODE
      ↕
SIMULATED CAN BUS
      ↕
SOCKETCAN
      ↕
USB CAN ADAPTER
      ↕
REAL CAN BUS
```

This should allow:

```text
simulated STM32
        ↕
simulated CAN
        ↕
real Teensy
```

and potentially:

```text
real ECU
   ↕
real CAN adapter
   ↕
CanLab
   ↕
simulated ECUs
```

Make the bridge clearly visible and require explicit user confirmation before transmitting onto physical hardware.

---

# 41. SAFETY FOR REAL HARDWARE

Physical CAN transmission is potentially consequential.

The application must:

* clearly distinguish simulation from hardware
* display the active physical interface
* require explicit enabling of physical transmission
* warn users before connecting a simulation to a real bus
* never silently transmit
* provide a prominent disconnect control
* prevent accidental project startup from transmitting unless explicitly configured

Example:

```text
⚠ REAL HARDWARE ENABLED

Interface:
can0

Bitrate:
500 kbit/s

CanLab is connected to a physical CAN network.

[Disconnect]
```

---

# 42. TESTING

Testing is mandatory.

Implement:

### Unit tests

For:

* CAN IDs
* frames
* DLC
* arbitration
* CRC
* bit stuffing
* timing
* error counters
* bus states
* filters

### Integration tests

Test:

```text
Node A → Bus → Node B
```

Test:

```text
STM32 → CAN → STM32
```

Test:

```text
Arduino → MCP2515 → CAN → STM32
```

Eventually:

```text
STM32 → CAN → Teensy
```

---

# 43. FIRMWARE TEST FIXTURES

Include minimal firmware projects.

Example:

```text
firmware/tests/stm32_sender
firmware/tests/stm32_receiver
firmware/tests/arduino_sender
firmware/tests/arduino_receiver
```

The CI system should eventually be able to run these automatically.

---

# 44. END-TO-END TEST

Create an automated test:

```text
Start simulation

Load STM32 sender firmware
Load STM32 receiver firmware

Connect both to CAN bus

Run simulation

Assert:
  receiver received ID 0x123
  payload == expected payload
  timestamp is valid
```

Then test arbitration:

```text
Node A sends 0x300
Node B sends 0x100

Assert:
  0x100 transmitted first
  Node A reports arbitration loss
```

---

# 45. PERFORMANCE

The architecture must not assume every simulation runs in real time.

Support:

* accelerated simulation
* deterministic simulation
* headless simulation
* batch testing

A headless project should be runnable:

```bash
canlab simulate vehicle.canlab
```

This should work without opening the GUI.

---

# 46. CLI

Provide:

```bash
canlab
canlab new
canlab open project.canlab
canlab simulate project.canlab
canlab validate project.canlab
canlab firmware ...
canlab doctor
```

`canlab doctor` should diagnose:

* Renode installation
* toolchains
* firmware support
* permissions
* SocketCAN
* hardware adapters

Example:

```text
CanLab Doctor

✓ Rust
✓ Node.js
✓ Renode
✓ STM32 toolchain
✓ AVR toolchain
✗ SocketCAN unavailable

System is ready for simulated CAN networks.
```

---

# 47. IMPORT/EXPORT

Support:

* project save/load
* CAN trace export
* CSV
* JSON
* PCAP/PCAPNG where practical
* firmware project references
* screenshots eventually

Do not embed huge firmware binaries into project files by default.

Use relative paths.

---

# 48. PROJECT PORTABILITY

A project should ideally be shareable as:

```text
my-project/
├── project.canlab
├── firmware/
│   ├── ecu.elf
│   └── dashboard.hex
├── dbc/
│   └── vehicle.dbc
└── assets/
```

Provide a packaging/export command eventually:

```bash
canlab package my-project
```

---

# 49. USER EXPERIENCE

The application should be approachable to someone familiar with Arduino/Tinkercad.

A new user should be able to:

1. create a project
2. drag STM32 onto canvas
3. drag Arduino onto canvas
4. add CAN bus
5. connect both
6. select firmware
7. press Run
8. open CAN Analyzer
9. see traffic

Avoid requiring users to understand Renode.

Renode should be an implementation detail.

---

# 50. ADVANCED MODE

Provide an advanced/debug mode exposing:

* backend logs
* Renode console
* CPU state
* peripheral registers where available
* memory
* CAN controller registers
* timing
* bus errors
* event trace

Do not clutter the beginner UI with this information.

---

# 51. DEBUGGING

Eventually support:

* breakpoints
* pause-on-CAN-frame
* pause-on-CAN-error
* register inspection
* memory inspection
* firmware logs
* backend logs

If Renode's debugger can be integrated, expose it through the abstraction.

---

# 52. EVENT SYSTEM

Implement an internal event bus.

Events should include concepts such as:

```text
SimulationStarted
SimulationPaused
SimulationStopped

NodeStarted
NodeStopped
NodeReset

CanFrameQueued
CanArbitrationStarted
CanArbitrationLost
CanFrameTransmitted
CanFrameReceived
CanError
CanBusOff

FaultInjected
```

The frontend should consume these events rather than polling everything.

---

# 53. OBSERVABILITY

Every CAN event should have a timestamp.

Allow event recording.

Eventually support:

```text
simulation recording
        ↓
save trace
        ↓
replay
```

This is useful for debugging.

---

# 54. REPLAY

Eventually allow:

```bash
canlab replay trace.json
```

or from the GUI:

```text
[Replay Recording]
```

Users should be able to replay CAN traffic deterministically.

---

# 55. EXTENSIBILITY

Components should be plugin-like internally even if external plugins aren't supported initially.

A device definition should conceptually specify:

```text
device ID
name
icon
backend
interfaces
configuration schema
```

For example:

```json
{
  "id": "stm32f103",
  "name": "STM32F103",
  "interfaces": [
    {
      "type": "can",
      "name": "CAN1"
    }
  ]
}
```

This makes future MCU support easier.

---

# 56. BACKEND-AGNOSTIC DESIGN

The project must never assume:

```text
STM32 = Renode
```

Instead:

```text
STM32
 ├── Renode backend
 ├── future QEMU backend
 └── future native backend
```

Likewise:

```text
Teensy
 ├── Renode
 └── future backend
```

---

# 57. DOCUMENTATION

Write documentation continuously.

Include:

```text
docs/
├── getting-started.md
├── architecture.md
├── can-model.md
├── mcu-backends.md
├── firmware.md
├── projects.md
├── fault-injection.md
├── dbc.md
├── socketcan.md
└── contributing.md
```

Explain both:

* how to use CanLab
* how to develop CanLab

---

# 58. OPEN SOURCE

Use a permissive license compatible with dependencies.

Before selecting the final license, inspect the licenses of all dependencies.

Maintain a dependency/license report.

Do not copy code from incompatible projects.

Do not assume that because a simulator is publicly available it can be forked or incorporated wholesale.

Prefer:

* APIs
* documented interfaces
* clean-room implementations where necessary
* compatible open-source dependencies

---

# 59. SECURITY

Treat:

* project files
* firmware
* backend output
* hardware interfaces

as potentially unsafe.

Do not execute arbitrary shell commands from firmware/project configuration.

Sandbox external processes where practical.

Provide clear warnings for physical hardware.

---

# 60. DEVELOPMENT PHASES

Follow this implementation sequence.

## Phase 0 — Architecture

Deliver:

* repository
* Rust workspace
* frontend shell
* project format
* architecture documentation
* CI

Do not build the complete GUI yet.

---

## Phase 1 — CAN Core

Implement:

* frame
* ID
* bus
* nodes
* arbitration
* timestamps
* basic transmission
* reception

Create extensive unit tests.

Goal:

```text
Node A → CAN Bus → Node B
```

---

## Phase 2 — Protocol correctness

Implement:

* CRC
* bit stuffing
* ACK
* error conditions
* error counters
* bus states
* timing

Add protocol tests.

---

## Phase 3 — CLI

Create:

```bash
canlab simulate
```

Allow a simple project file to run without the GUI.

Goal:

```text
STM32-like virtual node
      ↓
CAN
      ↓
STM32-like virtual node
```

---

## Phase 4 — Renode

Integrate Renode.

Get real STM32 firmware executing.

Goal:

```text
real STM32 firmware
        ↓
simulated STM32
        ↓
CAN
        ↓
simulated STM32
        ↓
real STM32 firmware
```

This is the first major success criterion.

---

## Phase 5 — GUI

Implement:

* canvas
* components
* connections
* project loading
* simulation controls

Do not overdesign.

---

## Phase 6 — CAN Analyzer

Implement:

* live frames
* filters
* frame inspection
* timestamps
* export

This should become one of the strongest parts of the application.

---

## Phase 7 — Arduino/MCP2515

Implement:

```text
ATmega328P
    ↓
SPI
    ↓
MCP2515
    ↓
CAN
```

Run real Arduino firmware.

---

## Phase 8 — Teensy

Implement:

```text
i.MX RT1062
    ↓
FlexCAN
    ↓
CAN
```

Run real Teensy firmware if the selected backend supports the necessary hardware.

If Renode does not adequately support the exact target, do NOT distort the architecture to force it.

Instead create a backend/device implementation plan.

---

## Phase 9 — Fault Injection

Add:

* dropped frames
* corrupted frames
* ACK errors
* CRC errors
* bus-off
* node failure
* timing faults

---

## Phase 10 — Waveforms

Add bit-level visualization.

---

## Phase 11 — CAN FD

Add Classical CAN-compatible extension architecture.

---

## Phase 12 — DBC

Add:

* import
* decoding
* signal visualization

---

## Phase 13 — SocketCAN

Add:

* virtual CAN
* Linux CAN integration
* CAN trace integration

---

## Phase 14 — Physical CAN

Add:

* USB CAN adapters
* hardware safety confirmation
* real/simulated mixed networks

---

# 61. MVP DEFINITION

Do NOT call the project MVP complete until all of these work:

### UI

* create project
* add STM32
* add Arduino
* add CAN bus
* connect nodes
* save project
* load project

### Firmware

* load STM32 firmware
* load Arduino firmware

### Simulation

* run/pause/stop/reset
* real firmware execution

### CAN

* standard frames
* 11-bit IDs
* 8-byte payload
* arbitration
* RX/TX
* timestamps
* basic errors

### Analyzer

* live traffic
* ID
* DLC
* payload
* sender
* timestamp
* filtering

### Backend

* Renode integration
* clean startup/shutdown
* useful errors

---

# 62. DO NOT IMPLEMENT YET

Explicitly defer:

* full SPICE
* PCB simulation
* every Arduino board
* every STM32
* every Teensy
* CAN FD before Classical CAN is stable
* physical CAN voltage simulation
* complete analog transceiver models
* full automotive network stack
* UDS
* J1939
* AUTOSAR
* graphical DBC editor
* cloud collaboration

These may be future features.

---

# 63. FUTURE AUTOMOTIVE FEATURES

Architect so these could eventually be added:

* UDS
* ISO-TP
* J1939
* CANopen
* LIN
* FlexRay
* SOME/IP
* automotive Ethernet
* DBC
* ARXML

Do not implement them in the MVP.

---

# 64. IMPORTANT ARCHITECTURAL DISTINCTION

Maintain three levels:

```text
Level 1
CAN FRAME

ID / DLC / DATA
```

```text
Level 2
CAN CONTROLLER

bxCAN
FlexCAN
MCP2515
```

```text
Level 3
PHYSICAL LAYER

CAN transceiver
CANH
CANL
termination
```

Do not collapse these layers.

This is essential for future realism.

---

# 65. PHYSICAL-LAYER FUTURE

Eventually allow:

```text
MCU
 ↓
CAN controller
 ↓
transceiver
 ↓
CANH/CANL
 ↓
termination
```

The initial CAN bus may operate at the logical frame/controller level.

Later introduce electrical behavior.

Do not make physical-layer simulation a requirement for basic CAN simulation.

---

# 66. PERFORMANCE ARCHITECTURE

Use asynchronous event processing where appropriate.

Avoid:

```text
frontend polls backend every millisecond
```

Prefer:

```text
Simulation
    ↓
Event stream
    ↓
WebSocket
    ↓
Frontend
```

Throttle UI rendering separately from simulation frequency.

The simulator must be capable of generating more events than the UI can render without becoming unstable.

---

# 67. LARGE TRAFFIC HANDLING

The analyzer must not store unlimited frames in memory.

Implement:

* ring buffers
* configurable capture limits
* disk-backed recording eventually
* filtering before rendering

The UI should remain responsive during high CAN traffic.

---

# 68. ERROR MESSAGES

Errors should be actionable.

Bad:

```text
Backend error.
```

Good:

```text
Could not start STM32F103 simulation.

Renode reported that the platform does not expose CAN1.

Device: stm32f103
Interface: CAN1

Check the selected platform/backend configuration.
```

---

# 69. DESIGN LANGUAGE

The UI should feel:

* technical
* clean
* modern
* lightweight
* developer-oriented

Do not make it look like a toy.

But it should still be approachable to Arduino users.

Use a clear visual distinction between:

* components
* connections
* active simulation
* CAN traffic
* errors
* physical hardware

---

# 70. ACCESSIBILITY

Support:

* keyboard navigation
* readable contrast
* scalable UI
* tooltips
* screen-reader-friendly controls where applicable
* non-color-only status indicators

---

# 71. CI/CD

Set up CI for:

* Rust formatting
* Rust linting
* unit tests
* frontend type checking
* frontend tests
* project schema validation

Eventually add integration CI for supported emulator environments.

Do not make Renode integration tests impossible to run locally.

---

# 72. DEVELOPMENT QUALITY

Do not produce a giant untested implementation.

Implement incrementally.

After each major subsystem:

1. write tests
2. run tests
3. document behavior
4. commit logically
5. verify integration

Avoid placeholder implementations that silently pretend to work.

If something isn't implemented, expose an explicit error.

For example:

```text
Teensy 4.1 is recognized but its current backend does not support FlexCAN simulation.
```

Do NOT silently emulate it as an STM32.

---

# 73. IMPLEMENTATION STRATEGY FOR AI CODING AGENTS

When implementing this project:

1. Inspect the existing repository before changing anything.
2. Preserve existing working code.
3. Do not rewrite large portions without justification.
4. Create tests before complicated protocol implementations.
5. Prefer small modules.
6. Keep public interfaces documented.
7. Avoid unnecessary dependencies.
8. Explain architectural decisions in code comments where non-obvious.
9. Never claim a feature works without testing it.
10. Never substitute a fake implementation for real MCU behavior without clearly labeling it.

If the repository already contains code, adapt this architecture to the existing project rather than blindly replacing it.

---

# 74. FIRST IMPLEMENTATION TASK

Start by implementing only:

```text
CanFrame
CanId
CanNode
CanBus
arbitration
simulation clock
```

Then create tests.

Required test:

```text
Node A → 0x300
Node B → 0x100
Node C → 0x200

Expected:

B wins arbitration.
A and C lose arbitration.
```

Required second test:

```text
Node A transmits:

ID: 0x123
DLC: 8
DATA:
01 02 03 04 05 06 07 08

Node B must receive exactly that frame.
```

Required third test:

```text
Node A transmits
Node B receives
Node C receives

All eligible nodes observe the winning frame.
```

Do not start the graphical UI until these tests pass.

---

# 75. SECOND IMPLEMENTATION TASK

Implement the CLI:

```bash
canlab simulate example.canlab
```

It should print:

```text
Starting CanLab...

Bus:
  vehicle_bus
  bitrate: 500000

Nodes:
  engine_ecu
  dashboard

Simulation started.

[0.001 ms] engine_ecu TX 0x123 [01 02 03 04]
[0.001 ms] dashboard RX 0x123 [01 02 03 04]
```

---

# 76. THIRD IMPLEMENTATION TASK

Integrate Renode.

Create the smallest possible working example:

```text
STM32F103 firmware A
        ↓
simulated CAN
        ↓
STM32F103 firmware B
```

Use actual firmware.

Do not fake the firmware API.

---

# 77. SUCCESS CRITERIA

The project is successful when a user can download CanLab, open it, create:

```text
STM32 ───── CAN BUS ───── Arduino
```

load real firmware into both devices, press:

```text
▶ Run
```

and observe actual CAN frames in:

```text
CAN Analyzer
```

without needing to know that Renode is being used internally.

The next major milestone is:

```text
STM32 ───── CAN ───── Arduino ───── CAN ───── Teensy
```

with all three using their respective real CAN-controller interfaces.

The long-term goal is:

```text
                 CANLAB

      ┌──────── simulated ────────┐
      │                            │
   STM32                         Teensy
      │                            │
      └────────── CAN BUS ─────────┘
                   │
                   │
              SocketCAN
                   │
                   ▼
              REAL CAN BUS
                   │
             ┌─────┴─────┐
             ▼           ▼
          real ECU    real Teensy
```

That is the ultimate product direction.

---

# 78. FINAL ENGINEERING RULE

**Do not optimize for "how quickly can I make something that looks like Tinkercad."**

Optimize for:

> **How quickly can I prove that real embedded firmware from different MCU families can communicate over a correctly modeled virtual CAN network?**

Once that works, the visual editor is an interface on top of a genuinely useful simulation engine.

The CAN engine should remain usable independently of the GUI.

The MCU abstraction should remain independent of Renode.

The project format should remain independent of the frontend.

The physical CAN layer should remain independent of the logical CAN layer.

And every future feature should fit into those boundaries rather than breaking them.
