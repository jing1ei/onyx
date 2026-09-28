#!/usr/bin/env bash
# Regenerate the per-format decode fixtures used by
# `crates/onyx-core/tests/format_coverage.rs` (SPEC §17).
#
# The fixtures are checked in - CI and other machines have no encoders - but
# they must be reproducible, so this script is the record of how they were
# made. Requires ffmpeg (7.x was used; any build with libmp3lame, libvorbis,
# libopus, aac and alac will do).
#
#   ./scripts/make-format-fixtures.sh
#
# Every fixture is the same 0.5 s programme: 440 Hz on the left leg, 660 Hz on
# the right, so a decode test can assert channel identity as well as rate,
# channel count and duration. They are deliberately tiny (a few kB each).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="$here/crates/onyx-core/tests/fixtures"
mkdir -p "$out"

# 440 Hz left / 660 Hz right, 0.5 s.
stereo() {
  local rate="$1"
  echo "-f lavfi -i sine=frequency=440:sample_rate=$rate:duration=0.5 \
        -f lavfi -i sine=frequency=660:sample_rate=$rate:duration=0.5 \
        -filter_complex [0:a][1:a]join=inputs=2:channel_layout=stereo[a] -map [a]"
}

gen() { # gen <output> <rate> <ffmpeg args...>
  local name="$1"; shift
  local rate="$1"; shift
  # shellcheck disable=SC2046
  ffmpeg -hide_banner -loglevel error -y $(stereo "$rate") "$@" "$out/$name"
  echo "  $(printf '%-22s' "$name") $(wc -c < "$out/$name") bytes"
}

echo "writing fixtures to $out"
gen tone.flac      44100 -c:a flac -sample_fmt s16 -compression_level 12
gen tone.mp3       48000 -c:a libmp3lame -b:a 64k
gen tone-aac.m4a   48000 -c:a aac -b:a 64k
gen tone-alac.m4a  44100 -c:a alac -sample_fmt s16p
gen tone.ogg       48000 -c:a libvorbis -q:a 1
gen tone.opus      48000 -c:a libopus -b:a 64k
# Uncompressed fixtures are generated at 16 kHz so the checked-in bytes stay
# small; the rate assertion then also proves an unusual rate survives probing.
gen tone.aiff      16000 -c:a pcm_s16be
gen tone.caf       16000 -c:a pcm_s16le
gen tone.mka       48000 -c:a libvorbis -q:a 1
gen tone.webm      48000 -c:a libopus -b:a 64k

# Video containers: a real (tiny) H.264 video track plus the same audio. The
# decoder must take the audio track and ignore the picture entirely.
video_and_audio() { # video_and_audio <name> <format>
  local name="$1"; shift
  ffmpeg -hide_banner -loglevel error -y \
    -f lavfi -i "color=c=black:s=64x64:r=10:d=0.5" \
    -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=0.5" \
    -f lavfi -i "sine=frequency=660:sample_rate=48000:duration=0.5" \
    -filter_complex "[1:a][2:a]join=inputs=2:channel_layout=stereo[a]" \
    -map 0:v -map "[a]" -c:v libx264 -preset veryfast -pix_fmt yuv420p \
    -c:a aac -b:a 64k "$@" "$out/$name"
  echo "  $(printf '%-22s' "$name") $(wc -c < "$out/$name") bytes"
}
video_and_audio tone-video.mp4 -f mp4
video_and_audio tone-video.mov -f mov

echo "done"
