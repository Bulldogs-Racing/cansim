// Parser fixture for `canlab sketch` (mcp_can dialect). Never compiled —
// there is no AVR toolchain here on purpose. Representative of real
// MCP2515 sketches: #defined ID, static payload with one dynamic byte.
#include <SPI.h>
#include <mcp_can.h>

#define CAN_ID 0x100
#define CAN_CS_PIN 10

MCP_CAN CAN(CAN_CS_PIN);

byte engineData[8] = {0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08};
byte counter = 0;

void setup() {
  while (CAN.begin(MCP_ANY, CAN_500KBPS, MCP_16MHZ) != CAN_OK) {
  }
  CAN.setMode(MCP_NORMAL);
}

void loop() {
  engineData[0] = counter++;          // dynamic byte (approximation boundary)
  CAN.sendMsgBuf(CAN_ID, 0, 8, engineData);
  delay(100);
}
