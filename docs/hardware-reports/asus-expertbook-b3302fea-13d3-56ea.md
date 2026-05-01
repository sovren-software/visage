# Hardware Report: ASUS ExpertBook B3302FEA/B5302FEA

**Date tested:** 2026-05-01
**Result:** Not compatible for secure IR-backed Visage authentication
**Validation level:** Read-only hardware probe; no UVC control writes were sent

## Summary

The ASUS ExpertBook B3302FEA/B5302FEA tested here exposes a single built-in
Azurewave/IMC UVC webcam (`13d3:56ea`). The camera is usable as a normal RGB
webcam, but it does not expose the hardware surface Visage needs for secure
Windows-Hello-style IR authentication:

- no separate IR video node
- no IR-oriented pixel format such as `GREY` or `Y16`
- no known Visage emitter quirk for `13d3:56ea`
- no Microsoft face-auth UVC extension unit in the USB descriptor
- no known Realtek/Windows-Hello emitter unit/selector layout

`/dev/video0` may be used for development or basic camera testing, but this
laptop should not be treated as Visage-compatible for PAM authentication unless a
future Windows USB capture proves a real IR emitter path and IR frame stream.
Use an external known-supported UVC IR camera for secure auth on this machine.

## Host

```text
Hostname: the-second
Vendor: ASUSTeK COMPUTER INC.
Product: ASUS EXPERTBOOK B3302FEA_B5302FEA
Board: B3302FEA
BIOS: B3302FEA.313
OS: Esver OS 1.0 (Aegis)
Kernel: Linux 7.0.0 x86_64
CPU: 11th Gen Intel(R) Core(TM) i7-1165G7
GPU: Intel TigerLake-LP GT2 [Iris Xe Graphics]
```

## Camera discovery

```text
Bus 003 Device 004: ID 13d3:56ea IMC Networks USB2.0 HD UVC WebCam

/dev/video0
  name=USB2.0 HD UVC WebCam: USB2.0 HD
  driver=uvcvideo
  idVendor=13d3
  idProduct=56ea
  manufacturer=Azurewave
  product=USB2.0 HD UVC WebCam
  udev: ID_V4L_CAPABILITIES=:capture:

/dev/video1
  name=USB2.0 HD UVC WebCam: USB2.0 HD
  driver=uvcvideo
  idVendor=13d3
  idProduct=56ea
  manufacturer=Azurewave
  product=USB2.0 HD UVC WebCam
  udev: ID_V4L_CAPABILITIES=:
```

`visage discover` reports no quirk:

```text
/dev/video0  driver=uvcvideo  VID=0x13d3 PID=0x56ea  no quirk (VID=0x13d3 PID=0x56ea)
/dev/video1  driver=uvcvideo  VID=0x13d3 PID=0x56ea  no quirk (VID=0x13d3 PID=0x56ea)
```

## V4L2 formats

`/dev/video0` exposes normal webcam formats only:

```text
[0]: 'MJPG' (Motion-JPEG, compressed)
  1280x720 @ 30fps
  640x480 @ 30fps
  352x288 @ 30fps
  320x240 @ 30fps
  176x144 @ 30fps
  160x120 @ 30fps

[1]: 'YUYV' (YUYV 4:2:2)
  1280x720 @ 10fps
  640x480 @ 30fps
  352x288 @ 30fps
  320x240 @ 30fps
  176x144 @ 30fps
  160x120 @ 30fps
```

`/dev/video1` is metadata only:

```text
Format Metadata Capture:
  Sample Format: 'UVCH' (UVC Payload Header Metadata)
```

No `GREY`, `Y8`, `Y10`, `Y12`, or `Y16` IR-like stream was present.

## USB descriptor notes

The device has one VideoControl interface, one VideoStreaming interface, and one
DFU firmware interface. It does not expose a second video interface for IR.

Extension units found:

```text
bUnitID=4
  guidExtensionCode={1229a78c-47b4-4094-b0ce-db07386fb938}
  bNumControls=2
  bControlSize=2
  bmControls=[0]=0x00, [1]=0x06

bUnitID=7
  guidExtensionCode={26b8105a-0713-4870-979d-da79444bb68e}
  bNumControls=2
  bControlSize=4
  bmControls=[0]=0x00, [1]=0x00, [2]=0x0c, [3]=0x00
```

Notably absent:

- Microsoft Extended Controls Unit / face-auth GUID
  `{0f3f95dc-2632-4c4e-92c9-a04782f43bc8}`
- Realtek emitter-style unit 14 / selector 14 pattern seen in some Howdy reports
- Visage's known ASUS Zenbook quirk path: unit 14 selector 6 for `04f2:b6d9`

## Read-only UVC extension-unit probe

Only `GET_*` queries were sent. No `SET_CUR` writes were sent.

```text
/dev/video0
  unit 4 selector 10: len=8, GET+SET, current=00 08 00 bb bb bb bb bb
  unit 4 selector 11: len=8, GET+SET, GET_CUR returned permission denied
  unit 7 selector 19: len=1, GET+SET, current=00
  unit 7 selector 20: len=10, GET+SET, current=00 00 00 00 00 00 00 00 00 00

/dev/video1
  no readable selectors on units 4, 7, or 14
```

These controls show vendor-specific state exists, but the descriptor and format
surface do not identify a supported IR emitter or IR frame stream. Without a
confirmed IR frame source, adding a quirk for this VID:PID would be unsafe and
misleading.

## Classification

Recommended compatibility classification:

```text
Tier: No IR camera / RGB-only UVC webcam
Visage support: Not compatible for secure PAM authentication
Testing-only camera path: /dev/video0
Quirk file: do not add for 13d3:56ea without Windows USB capture evidence
```

## If someone wants to continue reverse engineering

The next meaningful step is a Windows USB capture, not blind Linux brute force:

1. Boot Windows on the same laptop or attach the webcam to a Windows VM with USB
   passthrough.
2. Trigger Windows Hello or another known-good IR activation path.
3. Capture USB control transfers with USBPcap/Wireshark.
4. Filter out isochronous video traffic and inspect `SET_CUR` transfers to the
   camera.
5. Map `wIndex` and `wValue` to UVC unit/selector/payload bytes.
6. Only then test a local quirk experimentally.

Until that evidence exists, this hardware should remain documented as not
compatible with Visage's intended secure IR authentication path.
