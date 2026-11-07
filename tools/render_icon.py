#!/usr/bin/env python3
"""Export the original launcher artwork at native Windows/tray sizes (Pillow)."""
from pathlib import Path
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[1]
# Exact geometry and colors of packaging/linux/com.deskswitch.Desktop.svg.
# Supersampling only adds edge antialiasing; the established artwork is unchanged.
SCALE = 8
canvas = Image.new('RGBA', (128 * SCALE, 128 * SCALE))
draw = ImageDraw.Draw(canvas)
def rectangle(box, **options):
    draw.rectangle(tuple(value * SCALE for value in box), **options)
blue = '#5263e0'
draw.rounded_rectangle((10*SCALE,10*SCALE,118*SCALE,118*SCALE), radius=20*SCALE, fill=blue)
for left, top, width, height in [(27,33,44,35),(59,57,43,35)]:
    rectangle((left-2.5,top-2.5,left+width+2.5,top+height+2.5), fill='white')
    rectangle((left+2.5,top+2.5,left+width-2.5,top+height-2.5), fill=blue)
folder = ROOT / 'assets/icons'
folder.mkdir(parents=True, exist_ok=True)
sizes = [16,20,24,32,48,64,128,256]
for size in sizes:
    canvas.resize((size,size),Image.Resampling.LANCZOS).save(folder / f'desk-switch-{size}.png')
canvas.resize((256,256),Image.Resampling.LANCZOS).save(ROOT / 'assets/desk-switch.ico', sizes=[(size,size) for size in sizes])
print('Exported the original launcher artwork at:', ', '.join(map(str,sizes)))
