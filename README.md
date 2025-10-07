# z407-rotary-encabulator
# Controlling the Logitech Z407 Speakers via BLE

## Overview

This document outlines the reverse-engineered Bluetooth Low Energy (BLE) protocol for the Logitech Z407 wireless control puck. It enables custom control of volume, bass, source switching, and more. Based on community efforts, with bass control added from independent testing (prior work misinterpreted SOUND_2).

**Note:** Reverse-engineering may void warranties. Protocol is notification-only; no state queries. No security beyond basic handshake. AUX playback behavior unconfirmed.

## Use Cases

- Replace lost puck with apps/scripts (e.g., ESP32).
- Automate via events (e.g., switch inputs, adjust volume).
- Direct bass control without mode entry.

## BLE Connection

- **Service UUID:** `0000fdc2-0000-1000-8000-00805f9b34fb`
- **Command UUID (Writable):** `c2e758b9-0e78-41e0-b0cb-98a593193fc5`
- **Response UUID (Notifiable):** `b84ac9c6-29c5-46d4-bba1-9d534784330f`

## Protocol

1. Scan/connect to service.
2. Subscribe to responses.
3. Handshake: Send `0x84 0x05`; expect `d40501`; send `0x84 0x00`; expect `d40001` then `d40003`.
4. Send commands; receive confirmations.

## Puck Behavior

- **Volume Mode:** Twist for VOLUME_UP/DOWN; press for PLAY_PAUSE; double/triple for NEXT/PREV_TRACK.
- **Bass Mode:** Long-press (plays SOUND_2); twist for BASS_UP/DOWN; auto-exits after 15s.
- **Sources:** Wired toggle (USB/AUX); BT press (switch), long-press (pairing).

## Notes

- Switching pauses active streams.
- Volume needs active audio (ignores in sleep).
- Playback as media keys; works on BT/USB (AUX unconfirmed).
- BT multi-point: First streaming device prioritized.
- Use tools like nRF Connect for testing.

## Command Reference

### Handshake

| Name       | Hex       |
|------------|-----------|
| INITIATE   | 0x84 0x05 |
| ACKNOWLEDGE| 0x84 0x00 |

### User Commands

| Name              | Hex       | Description                          |
|-------------------|-----------|--------------------------------------|
| VOLUME_UP         | 0x80 0x02 | Increase volume                      |
| VOLUME_DOWN       | 0x80 0x03 | Decrease volume                      |
| BASS_UP           | 0x80 0x00 | Increase bass                        |
| BASS_DOWN         | 0x80 0x01 | Decrease bass                        |
| PLAY_PAUSE        | 0x80 0x04 | Toggle play/pause                    |
| NEXT_TRACK        | 0x80 0x05 | Next track                           |
| PREV_TRACK        | 0x80 0x06 | Previous track                       |
| SWITCH_BLUETOOTH  | 0x81 0x01 | Switch to Bluetooth                  |
| SWITCH_AUX        | 0x81 0x02 | Switch to AUX                        |
| SWITCH_USB        | 0x81 0x03 | Switch to USB                        |
| SOUND_1           | 0x85 0x01 | Failure chime                        |
| SOUND_2           | 0x85 0x02 | Mode switch chime                    |
| SOUND_3           | 0x85 0x03 | Connection chime                     |
| PAIRING           | 0x82 0x00 | Enter pairing mode                   |
| FACTORY_RESET     | 0x83 0x00 | Reset to defaults                    |

### Unknown

| Name      | Hex       | Description              |
|-----------|-----------|--------------------------|
| UNKNOWN_1 | 0x85 0x00 | Has no noticeable effect.|

## Response Reference

| Name               | Hex     | Trigger                             |
|--------------------|---------|-------------------------------------|
| INITIATE_RESPONSE  | d40501  | INITIATE                            |
| ACKNOWLEDGE_RESPONSE| d40001 | ACKNOWLEDGE                         |
| CONNECTED          | d40003  | Post-handshake                      |
| BASS_UP            | c000    | BASS_UP                             |
| BASS_DOWN          | c001    | BASS_DOWN                           |
| VOLUME_UP          | c002    | VOLUME_UP                           |
| VOLUME_DOWN        | c003    | VOLUME_DOWN                         |
| PLAY_PAUSE         | c004    | PLAY_PAUSE                          |
| NEXT_TRACK         | c005    | NEXT_TRACK                          |
| PREV_TRACK         | c006    | PREV_TRACK                          |
| SWITCH_BLUETOOTH   | c101    | SWITCH_BLUETOOTH                    |
| SWITCH_AUX         | c102    | SWITCH_AUX                          |
| SWITCH_USB         | c103    | SWITCH_USB                          |
| PAIRING            | c200    | PAIRING                             |
| FACTORY_RESET      | c300    | FACTORY_RESET                       |
| SOUND_1            | c503    | SOUND_1                             |
| SOUND_2            | c502    | SOUND_2                             |
| SOUND_3            | c501    | SOUND_3                             |
| UNKNOWN_1          | c500    | UNKNOWN_1                           |
| SWITCHED_BLE       | cf04    | Bluetooth switch complete (if changed)|
| SWITCHED_AUX       | cf05    | AUX switch complete (if changed)    |
| SWITCHED_USB       | cf06    | USB switch complete (if changed)    |

## Citations

- [freundTech/logi-z407-reverse-engineering](https://github.com/freundTech/logi-z407-reverse-engineering)