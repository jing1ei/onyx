#!/usr/bin/env python3
"""Render the pink-blue glass icon. Requires Pillow and NumPy.

Run this script to regenerate source, PNG sizes, ICO, and ICNS files.
The README uses the 128 px PNG.
"""
from pathlib import Path
import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageChops

SIZE = 1024
SCALE = 2
N = SIZE * SCALE
ROOT = Path(__file__).resolve().parent.parent


def mask_rect(box, radius):
    mask = Image.new('L', (N, N))
    ImageDraw.Draw(mask).rounded_rectangle(tuple(int(x * SCALE) for x in box),
                                          radius=int(radius * SCALE), fill=255)
    return mask


def gradient(stops, diagonal=True):
    y, x = np.mgrid[0:N, 0:N].astype(np.float32) / (N - 1)
    t = (x * .56 + y * .44) if diagonal else y
    colors = np.array([c for _, c in stops], dtype=np.float32)
    positions = np.array([p for p, _ in stops])
    rgb = np.stack([np.interp(t, positions, colors[:, c]) for c in range(3)], axis=-1)
    return Image.fromarray(np.uint8(np.clip(rgb, 0, 255)), 'RGB').convert('RGBA')


def layer(image, color, mask):
    fill = Image.new('RGBA', (N, N), color)
    fill.putalpha(mask)
    image.alpha_composite(fill)


def render():
    image = Image.new('RGBA', (N, N))
    tile = mask_rect((56, 48, 968, 960), 205)
    shadow = ImageChops.offset(tile, 0, 18 * SCALE).filter(ImageFilter.GaussianBlur(18 * SCALE))
    layer(image, (80, 107, 161), shadow.point(lambda v: int(v * .20)))
    body = gradient([(0, (255, 234, 245)), (.40, (245, 237, 254)), (1, (172, 216, 248))])
    body.putalpha(tile)
    image.alpha_composite(body)

    # Soft interior refraction and a fine bevel; silhouette remains simple.
    glow = Image.new('L', (N, N))
    ImageDraw.Draw(glow).ellipse((-160*SCALE, -150*SCALE, 830*SCALE, 540*SCALE), fill=160)
    glow = ImageChops.multiply(glow.filter(ImageFilter.GaussianBlur(90*SCALE)), tile)
    layer(image, (255, 255, 255), glow)
    inner = mask_rect((64, 56, 960, 952), 198)
    edge = ImageChops.subtract(tile, inner)
    layer(image, (255, 255, 255), edge.point(lambda v: int(v*.82)))

    mark = Image.new('L', (N, N))
    d = ImageDraw.Draw(mark)
    box = tuple(v*SCALE for v in (225, 213, 799, 787))
    d.arc(box, 103, 257, fill=255, width=82*SCALE)
    d.arc(box, -77, 77, fill=255, width=82*SCALE)
    d.rounded_rectangle((475*SCALE, 331*SCALE, 549*SCALE, 669*SCALE), radius=37*SCALE, fill=255)

    # Colored cast shadow gives the translucent symbol depth.
    cast = ImageChops.offset(mark, 0, 13*SCALE).filter(ImageFilter.GaussianBlur(12*SCALE))
    layer(image, (104, 102, 159), cast.point(lambda v: int(v*.32)))
    glass = gradient([(0, (255, 202, 222)), (.28, (240, 140, 187)),
                      (.52, (171, 162, 221)), (.72, (105, 168, 225)), (1, (58, 126, 196))])
    glass.putalpha(mark)
    image.alpha_composite(glass)

    # Highlight upper edges, shaded lower edges, and a broad reflected light.
    light = ImageChops.subtract(mark, ImageChops.offset(mark, 0, 5*SCALE))
    layer(image, (255, 250, 255), light.filter(ImageFilter.GaussianBlur(.8*SCALE)).point(lambda v: int(v*.90)))
    dark = ImageChops.subtract(mark, ImageChops.offset(mark, 0, -5*SCALE))
    layer(image, (92, 105, 175), dark.point(lambda v: int(v*.38)))
    reflection = Image.new('L', (N, N))
    ImageDraw.Draw(reflection).polygon([(100*SCALE, 260*SCALE), (850*SCALE, 110*SCALE),
                                       (910*SCALE, 390*SCALE), (90*SCALE, 590*SCALE)], fill=95)
    reflection = ImageChops.multiply(reflection.filter(ImageFilter.GaussianBlur(18*SCALE)), mark)
    layer(image, (255, 255, 255), reflection)
    return image.resize((SIZE, SIZE), Image.Resampling.LANCZOS)


def main():
    icon = render()
    out = ROOT / 'src-tauri/icons'
    out.mkdir(parents=True, exist_ok=True)
    icon.save(out / 'source.png', optimize=True)
    for size, name in [(32,'32x32.png'), (64,'64x64.png'), (128,'128x128.png'),
                       (256,'128x128@2x.png'), (512,'icon.png')]:
        icon.resize((size,size), Image.Resampling.LANCZOS).save(out/name, optimize=True)
    icon.save(out/'icon.ico', sizes=[(16,16),(24,24),(32,32),(48,48),(64,64),(128,128),(256,256)])
    icon.save(out/'icon.icns')
    print(f'Generated icon source and macOS/Windows assets in {out}')


if __name__ == '__main__':
    main()
