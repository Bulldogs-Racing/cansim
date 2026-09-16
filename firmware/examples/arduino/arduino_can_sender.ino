// Parser fixture for `canlab sketch` (sandeepmistry arduino-CAN dialect).
// Never compiled — there is no AVR toolchain here on purpose.
// Representative packet open/write/close shape with a #defined ID.
#include <CAN.h>

#define DASH_ID 0x200

byte dashData[4] = {0x20, 0x01, 0x00, 0x00};

void setup() {
  if (!CAN.begin(500E3)) {
    while (1) {
    }
  }
}

void loop() {
  CAN.beginPacket(DASH_ID);
  CAN.write(dashData, sizeof(dashData));
  CAN.endPacket();
  delay(200);
}
