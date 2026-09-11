# FFmpeg / FFprobe

This Windows distribution includes unmodified ffmpeg.exe and ffprobe.exe from Gyan Doshi's FFmpeg 8.0 full build. Onyx invokes these separate command-line programs for audio conversion and inspection.

- Version: 8.0-full_build-www.gyan.dev
- License: GNU GPL version 3; see LICENSE-GPLv3.txt (installed as FFmpeg-LICENSE-GPLv3.txt).
- Binary provider: https://www.gyan.dev/ffmpeg/builds/
- Package: https://github.com/GyanD/codexffmpeg/releases/download/8.0/ffmpeg-8.0-full_build.7z
- FFmpeg source revision: https://github.com/FFmpeg/FFmpeg/commit/140fd653ae
- FFmpeg source archive: https://github.com/MoeCici/Onyx_Branch/releases/download/editor-1.0.0-20260911-full/FFmpeg-8.0-140fd653ae-source.tar.gz
- Build configuration and upstream source information: UPSTREAM-README.txt (installed as FFmpeg-UPSTREAM-README.txt). External library information is maintained by the binary provider at https://www.gyan.dev/ffmpeg/builds/#libraries .

The source archive above contains the corresponding FFmpeg project source. It is not a source archive of every external library in the provider's full build. Refer to the provider's build/source information for those components.

FFmpeg and its included third-party components retain their respective copyrights and licenses. FFmpeg is a trademark of Fabrice Bellard. No restrictions on inspection, modification or reverse engineering of these tools are added by Onyx.

The authoritative per-file SHA-256 values are checked by scripts/prepare-ffmpeg.ps1 and included in the release checksum file.
