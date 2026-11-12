# 3D preview with BlenderDMX

Blimey mirrors its DMX output over Art-Net; [BlenderDMX](https://blenderdmx.eu), a Blender add-on, receives it and lights a 3D model of the rig. Nothing in Blimey's UI is involved, and the preview is optional.

## Install

1. Blender 4.2 or newer.
2. Install the "DMX" add-on from the [Blender Extensions platform](https://extensions.blender.org/add-ons/open-stage-blender-dmx): drag it into Blender, or download the zip and use Install from Disk.
3. In the 3D View, open the sidebar (`N`), go to the DMX tab and click Create New Show.

## Receive Art-Net

1. In the DMX tab, create one universe and set its input to Art-Net. BlenderDMX numbers Art-Net universes from 0, as Blimey does: its universe 0 is `--artnet-universe 0`.
2. Leave the Art-Net IP setting at its default, `0.0.0.0`, and enable Art-Net input.
3. Run Blimey with the mirror on. `--artnet` alone sends to 127.0.0.1 on universe 0:

   ```sh
   blimey run shows/fossile --dmx-port /dev/cu.usbserial-… --artnet
   blimey fire shows/fossile --set all-wash color=deep-blue dimmer=60% --artnet
   ```

   For BlenderDMX on another computer, give its address: `--artnet 192.168.1.20`.

BlenderDMX's docs ask to start it before any other Art-Net program on the same computer. That concerns programs that also listen on UDP port 6454, such as ENTTEC EMU. Blimey only sends, from a free port, so it can start before or after Blender. If nothing lights up, set Setup > Logging to Debug in the DMX tab and start Blender from a terminal to see the log.

## Patch the washes

Add each fixture with Fixture > Add, on universe 0, at the addresses in `shows/fossile/rig.yaml` (1, 17, 33 and 49 at the time of writing), in 16-channel mode.

There is no starter Blender scene for the 4-wash rig, because it needs a GDTF profile for the Stairville Wild Wash Pro 648 RGBW and none could be found. GDTF Share's library is behind a login and we could not confirm a file there, and none of BlenderDMX's built-in profiles matches the wash's 16-channel mode (four RGBW segments). To get one, either:

- log in to [GDTF Share](https://gdtf-share.com) and search for "Wild Wash Pro 648"; or
- build a profile with the GDTF Builder on the same site, following the channel table in `shows/fossile/fixtures/stairville-wild-wash-pro-648-rgbw.yaml`, which was checked against the manual.

Then import it with Setup > Import > Import GDTF Profile and pick it when adding the fixtures.

## Pixel grids on other universes

The same Art-Net output carries pixel grids that don't fit the Enttec universe (plan §7). Once C9 renders them, each grid's universe arrives in BlenderDMX like the washes' universe: add a BlenderDMX universe set to Art-Net for each one.
