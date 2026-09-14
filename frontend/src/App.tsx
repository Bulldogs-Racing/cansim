import React from "react";

/**
 * Phase 5 placeholder: the visual editor will be a network-topology canvas
 * (React Flow) over the Rust simulation engine. Per §78 the engine stays
 * authoritative — this UI only edits projects and renders the SimEvent
 * stream. No simulation logic belongs here.
 */
export default function App(): JSX.Element {
  return (
    <main style={{ fontFamily: "system-ui, sans-serif", padding: 24 }}>
      <h1>CanLab</h1>
      <p>
        Visual CAN network editor — coming in Phase 5. The deterministic
        simulation engine already runs headless:
      </p>
      <pre>
        <code>canlab simulate examples/two_nodes.canlab.yaml</code>
      </pre>
      <p>Planned canvas nodes: STM32F103 · Arduino Uno · Teensy 4.1 · MCP2515 · CAN Bus · CAN Analyzer.</p>
    </main>
  );
}
