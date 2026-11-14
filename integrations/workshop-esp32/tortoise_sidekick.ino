// ============================================================================
// G-III ("The Tortoise") — Physical Workshop Sidekick Firmware
// Target: ESP32 / Arduino Nano / STM32 BluePill (115200 8N1 over FTDI/CP2102)
// ============================================================================
// Wiring (USB-to-UART Adapter <-> Microcontroller <-> Workshop Iron):
//   - FTDI/CP2102 TX  ---> MCU RX
//   - FTDI/CP2102 RX  ---> MCU TX
//   - FTDI/CP2102 GND ---> MCU GND
//   - PIN_RELAY_CONTACTOR : Drives main battery pack contactor / relay
//   - PIN_PWM_FRONT       : Front hub / mid-drive ESC throttle PWM signal
//   - PIN_PWM_REAR        : Rear torque-plate ESC throttle PWM signal
//   - PIN_ADC_THROTTLE_F  : Physical Front split-throttle Hall sensor (0-3.3V/5V)
//   - PIN_ADC_THROTTLE_R  : Physical Rear split-throttle Hall sensor (0-3.3V/5V)
//   - PIN_TRIGGER_WIRE    : Pushbutton to fire "workshop\n" / "wire\n" into Ring-0
// ============================================================================

#include <Arduino.h>

#if defined(ESP32)
  const int PIN_STATUS_LED      = 2;
  const int PIN_RELAY_CONTACTOR = 26;
  const int PIN_PWM_FRONT       = 25;
  const int PIN_PWM_REAR        = 27;
  const int PIN_ADC_THROTTLE_F  = 34;
  const int PIN_ADC_THROTTLE_R  = 35;
  const int PIN_TRIGGER_BTN     = 0;
#else
  const int PIN_STATUS_LED      = 13;
  const int PIN_RELAY_CONTACTOR = 4;
  const int PIN_PWM_FRONT       = 5;
  const int PIN_PWM_REAR        = 6;
  const int PIN_ADC_THROTTLE_F  = A0;
  const int PIN_ADC_THROTTLE_R  = A1;
  const int PIN_TRIGGER_BTN     = 7;
#endif

static String rxLine = "";
static unsigned long lastButtonMs = 0;

void parseKernelTelemetryFrame(const String &line) {
  // Example frame emitted by `workshop` / `ebike` in kernel/src/shell.rs:
  // [G3:TELEM V=67200mV W=7392 F=79% R=95% RELAY=1 DERATE=0]
  int fIdx = line.indexOf("F=");
  int rIdx = line.indexOf("R=");
  int relIdx = line.indexOf("RELAY=");

  if (fIdx > 0 && rIdx > 0 && relIdx > 0) {
    int frontPct = line.substring(fIdx + 2, line.indexOf('%', fIdx)).toInt();
    int rearPct  = line.substring(rIdx + 2, line.indexOf('%', rIdx)).toInt();
    int relayOn  = line.substring(relIdx + 6, relIdx + 7).toInt();

    digitalWrite(PIN_RELAY_CONTACTOR, relayOn ? HIGH : LOW);
    analogWrite(PIN_PWM_FRONT, map(constrain(frontPct, 0, 100), 0, 100, 0, 255));
    analogWrite(PIN_PWM_REAR,  map(constrain(rearPct,  0, 100), 0, 100, 0, 255));
    digitalWrite(PIN_STATUS_LED, HIGH);
  }
}

void setup() {
  pinMode(PIN_STATUS_LED, OUTPUT);
  pinMode(PIN_RELAY_CONTACTOR, OUTPUT);
  pinMode(PIN_PWM_FRONT, OUTPUT);
  pinMode(PIN_PWM_REAR, OUTPUT);
  pinMode(PIN_TRIGGER_BTN, INPUT_PULLUP);

  digitalWrite(PIN_RELAY_CONTACTOR, LOW);
  analogWrite(PIN_PWM_FRONT, 0);
  analogWrite(PIN_PWM_REAR, 0);

  // Matches G-III bare-metal UART 16550 COM1 (0x3F8) @ 115200 8N1
  Serial.begin(115200);
  rxLine.reserve(160);
}

void loop() {
  // 1. Listen to Ring-0 COM1 (0x3F8) Umbilical Stream
  while (Serial.available() > 0) {
    char c = (char)Serial.read();
    if (c == '\n' || c == '\r') {
      if (rxLine.length() > 0) {
        if (rxLine.indexOf("[G3:TELEM") >= 0) {
          parseKernelTelemetryFrame(rxLine);
        } else if (rxLine.indexOf("[1/3 ZERO-COPY PIPELINE]") >= 0) {
          digitalWrite(PIN_STATUS_LED, !digitalRead(PIN_STATUS_LED));
        }
        rxLine = "";
      }
    } else if (rxLine.length() < 150) {
      rxLine += c;
    }
  }

  // 2. Physical Workbench Trigger -> Injects command directly into `spatial>` Ring-0 shell
  if (digitalRead(PIN_TRIGGER_BTN) == LOW && (millis() - lastButtonMs) > 500) {
    lastButtonMs = millis();
    Serial.print("workshop\n");
  }
}
