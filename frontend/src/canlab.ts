/** Shared project-shape types (mirror of src/project/schema.rs, version 1). */
export interface CanLabProject {
  version: 1;
  simulation: { mode: "deterministic" | "realtime" };
  buses: Array<{ id: string; type: "can"; bitrate: number; fd: false }>;
  nodes: Array<{
    id: string;
    device: "stm32f103" | "arduino_uno" | "teensy41" | "mcp2515" | "can_analyzer" | "generic_can_node";
    backend: "virtual" | "renode";
    firmware?: string;
    can: { bus: string };
  }>;
}

/** One row of the future CAN analyzer table (§28). */
export interface AnalyzerRow {
  timeNs: number;
  node: string;
  id: string;
  dlc: number;
  data: string;
}
