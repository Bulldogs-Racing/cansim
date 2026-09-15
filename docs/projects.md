# Projects (`project.canlab`)

Versioned, human-readable YAML (JSON also parses — JSON is a subset of
YAML 1.2). Current version: **1**. Parsers reject any other version so
future formats can evolve without silent misreads.

## Minimal example

```yaml
version: 1
simulation:
  mode: deterministic     # or: realtime
buses:
  - id: vehicle_bus
    type: can             # only "can" today
    bitrate: 500000
    fd: false             # must be false until CAN FD (Phase 11)
nodes:
  - id: engine_ecu
    device: stm32f103     # stm32f103 | arduino_uno | teensy41 | mcp2515 | can_analyzer | generic_can_node
    backend: virtual      # virtual (runs now) | renode (validates now, executes in Phase 4)
    firmware: ./firmware/engine.elf
    can:
      bus: vehicle_bus
```

## Headless-traffic extension

`messages:` (optional) scripts virtual-node frames for CI/headless runs:

```yaml
messages:
  - sender: engine_ecu
    id: 0x123
    data: [0x01, 0x02, 0x03, 0x04]
    extended: false       # default; true = 29-bit ID
```

Backwards compatible: projects without it run the built-in demo frame.

## Safety rules (§21)

Project files are untrusted input. Validation enforces:

- unique, non-empty bus/node ids; nodes must attach to a declared bus
- `fd: true` rejected (explicit "not implemented", never silent)
- firmware paths must be **relative**, contain no `..`, no shell
  metacharacters (`; | & $ \``); missing files are warnings (virtual runs
  don't need real ELF/HEX yet)
- no project content ever executes as a shell command

## Portability (§48)

Share a project as a directory (`project.canlab` + relative
`firmware/` + `dbc/` + `assets/`). Never embed huge binaries in the
project file — reference them by relative path.

`canlab package project.canlab --out pkg/` builds that directory for
you: it validates, then copies the project file, every referenced
firmware file (missing firmware is a hard error here — packages must be
complete), and the `dbc/`/`assets/` companions when present, preserving
relative layout so the packaged project validates and simulates from
its new home. The output directory must not exist yet.
