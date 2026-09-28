# Using Onyx

[← Overview](../README.md)

## Install and open

**Mac:** use the universal `.dmg` on Intel or Apple silicon, macOS Big Sur 11.3
or later. Drag Onyx to Applications. Unsigned downloads may require approval
through the system's Open / Privacy & Security flow; only approve a build you
trust. Keep Big Sur's Safari/WebKit updates installed.

**Windows:** use the x64 setup `.exe` on Windows 10/11. The installer registers
file associations and installs or updates WebView2 if needed (internet required).
The bare `.exe` does neither. For offline setup, preinstall the Evergreen
Standalone WebView2 Runtime. Unsigned builds may trigger SmartScreen.

## Open, add, and choose tracks

- **Open files** or **⌘/Ctrl+O** replaces the current playlist and starts the
  first track. The native file picker selects files; drop folders into the window
  to include their supported audio files recursively.
- **Add**, **⌘/Ctrl+Shift+O**, and a window-level drop append files. Existing
  playback continues; when no deck is loaded, the first added file starts.
- Click a playlist row to load and play it from the start on the current deck
  (A while comparison is off). The other deck and comparison switch stay unchanged.
  ↑/↓ and Home/End select; Enter or Space plays. Previous/Next follow the current deck.
- The row's **A** and **B** chips assign a deck. You can also drag the row onto
  a waveform, use Shift+A/Shift+B, or use the right-click menu. Assigning a deck
  preserves the comparison switch. Enable **A/B** manually when ready; it starts
  off on every app launch.
- Drag rows to reorder. Delete removes the selected row; **Clear** empties the
  playlist. Deck-changing actions are locked during a blind test.

Supported inputs include WAV/BWF, FLAC, MP3, AAC/M4A, ALAC, Ogg Vorbis, Opus,
AIFF/AIFC, CAF, Matroska/WebM, and the audio track of MOV/MP4. MIDI files render
through the bundled GeneralUser GS SoundFont or a custom `.sf2` selected in
Settings. Zip archives expand into playable entries; nested zips are not opened.

## Compare and loop

Turn on **A/B** before using B or Tab to switch. A and B share a playhead.
Switching decks changes the audible material without
restarting it. Click a waveform to select its deck and seek. Shift-drag sets a
loop region; **L** toggles looping.

**Level match** is opt-in and only attenuates the louder track. It waits for
analysis if no cached measurement is available. The displayed trim shows the
adjustment; switching it off restores unity trim.

**Auto** estimates a constant offset for B relative to A. If it cannot find a
confident match, the offset stays unchanged. Use the nudge buttons or Alt-drag
B for manual alignment; **Reset** returns to zero. It cannot correct a tempo
change or different edits. Offsets are limited to ±30 seconds.

## Read the meters

- **LUFS I:** integrated loudness. **M / S:** momentary and short-term loudness.
- **TP:** true peak. **LRA:** loudness range. Correlation describes the stereo
  relationship; the level meters show signal level.
- The monitor folds audition mono, side, left/right, swapped channels, or right
  polarity inversion. Press a fold's key again to return to stereo.
- Meters describe the signal after EQ, before band solo and the monitor fold.
- Bit-transparent playback requires matching source/engine rates, unity volume
  and trim, no mute/inversion, transparent EQ, no band solo, and stereo monitoring.
  The badge reports the current path; it is not a claim about the OS or hardware.

## Equaliser

Press **E** to open the separate EQ window and enable EQ, including when
reopening a bypassed setup. Opening keeps the existing bands. Click empty graph space to add a
band, drag a node to adjust, right-click it for filter choices, Alt-click to
bypass it, or double-click to remove it. Hold ⌘/Ctrl and drag to solo a frequency
range; horizontal movement chooses frequency, vertical movement changes Q.

**EQ ON** is highlighted in the main window whenever EQ and at least one band
are enabled. It stays visible after closing the editor. **Shift+E** bypasses the
whole EQ and removes the highlight. Click **EQ ON / EQ OFF** in the main player
to toggle processing without opening the editor; the adjacent EQ icon opens it.
The editor shows processing status but has no global bypass button. Closing the EQ window stops the spectrum and
solo sweep but retains the EQ processing. **Float** controls always-on-top.

## Blind listening

Assign A and B, manually enable **A/B**, then prepare level matching and alignment if wanted, before
starting. **Blind** is available even before setup is complete and explains
what is missing.

- **ABX:** A and B are references; X is randomly one of them on each trial.
  Listen with A/B/X, then vote **X = A** or **X = B** (keys 1/2).
- **A/B:** X and Y hide the two decks, with a new mapping each trial. Vote for
  the slot containing deck A (keys 1/2).
- You may pause or loop while listening. Finish the trials for the score,
  per-trial answers, and exact binomial p-value. A single run is evidence for
  that comparison under those conditions, not a guarantee of future results.
- **Abort** or the panel's close button ends an active run. **New test** returns
  to protocol/trial choices; **Run again** repeats the setup.

## Settings and appearance

Settings controls the appearance, audio host/device/rate/buffer, MIDI SoundFont,
and loudness cache. Device changes can rebuild the stream and re-decode tracks.
Actual rate and latency depend on the selected device.

New installations and Reset Appearance use Morandi warm stone, mist blue, and dusty rose. Saved theme
choices stay unchanged. Use **Simple** for theme, accent, fonts, and size. Use **Theme code** to copy a
default, import JSONC, validate, inspect contrast warnings, and apply. The separate
editor keeps a readable designed appearance even if the document is unreadable.

**⌘/Ctrl+Alt+Shift+R** resets appearance in any window. Escape leaves the theme
text field; a second Escape closes its panel/window. The Settings close button
remains reachable when scrolling. The playlist is not restored after quitting.

## If something goes wrong

| Symptom | Next step |
|---|---|
| No sound | Check Play, Mute, Volume, the selected output device, and monitor fold. Return to stereo and bypass EQ to isolate the cause. |
| B is empty | Click B beside the second playlist track, or drop a file onto B. |
| Blind test cannot begin | Load both decks. Choose tracks returns you to the playlist. |
| File is unavailable | Check that it still exists and is readable; open it again. A warning names malformed or unsupported media. |
| Track ends early | Check the truncation warning; decoded audio may have reached the memory budget. |
| Engine connection failed | Click Reconnect to reload the window. |
| Theme is unreadable | Press ⌘/Ctrl+Alt+Shift+R, or reset from the separate theme editor. |
| App reports a render error | Use Reload the window; include the log when reporting the issue. |

Logs: `~/Library/Logs/com.onyxaudio.player/onyx.log` on macOS;
`%LOCALAPPDATA%\com.onyxaudio.player\logs\onyx.log` on Windows. Logs rotate and
normally use warning level. Inspect logs before sharing, especially if debug
logging was enabled. Report security issues using [SECURITY.md](../SECURITY.md).
