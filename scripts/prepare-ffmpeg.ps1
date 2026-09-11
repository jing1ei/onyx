param([Parameter(Mandatory=$true)][string]$FFmpegDirectory)
$ErrorActionPreference='Stop'
$source=(Resolve-Path -LiteralPath $FFmpegDirectory).Path
$destination=Join-Path (Split-Path $PSScriptRoot -Parent) 'vendor/ffmpeg'
$hashes=@{
    'ffmpeg.exe'='81B5832F6C548D64FEBA0EC7D643397E2351D9F3460A65C6C296956045EEBAEA'
    'ffprobe.exe'='421DBC81A3D758DFE114534FF48EF47A61EF91896EB4B9E819609A8752AB8272'
}
foreach($name in $hashes.Keys){
    $file=Join-Path $source "bin/$name"
    if((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -ne $hashes[$name]){throw "Unexpected FFmpeg 8.0 binary: $name"}
}
New-Item -ItemType Directory -Force (Join-Path $destination 'bin') | Out-Null
foreach($name in $hashes.Keys){Copy-Item -LiteralPath (Join-Path $source "bin/$name") -Destination (Join-Path $destination "bin/$name")}
Copy-Item -LiteralPath (Join-Path $source 'LICENSE') -Destination (Join-Path $destination 'LICENSE-GPLv3.txt')
Copy-Item -LiteralPath (Join-Path $source 'README.txt') -Destination (Join-Path $destination 'UPSTREAM-README.txt')
Write-Output 'FFmpeg/FFprobe 8.0 verified and prepared.'
