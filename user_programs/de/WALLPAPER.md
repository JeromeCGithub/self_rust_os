# Default space wallpaper

**NIRCam Image of the “Cosmic Cliffs” in Carina**

Credit: **NASA, ESA, CSA, and STScI**.

- [Original image and credit](https://esawebb.org/images/weic2205a/)
- [Publication JPEG](https://cdn.esawebb.org/archives/images/publicationjpg/weic2205a.jpg)
- [ESA/Webb usage terms](https://esawebb.org/copyright/)
- [Creative Commons Attribution 4.0 International](https://creativecommons.org/licenses/by/4.0/)

The image is cropped to 16:9, resized to 1920×1080 with Lanczos resampling, and
stored as an RGB P6 PPM for the native desktop. The colors are not recolored.
The DE displays the full credit in its bottom panel. No ESA, NASA or Webb logo
is included, and no endorsement is implied.

Source JPEG SHA-256:
`0c36eb0d1d7ccb097652895e9597cc0678a35148ab494697a387f1a55889230f`

Installed `wallpaper.ppm` SHA-256:
`7a3cac0483cfdd27153cf1d1b3ca2698b07f8de57cf5a10843d5d04210b594cd`

To reproduce from the downloaded publication JPEG using Pillow:

```python
from pathlib import Path
from PIL import Image, ImageOps

with Image.open("weic2205a.jpg") as source:
    image = ImageOps.fit(source.convert("RGB"), (1920, 1080),
                         method=Image.Resampling.LANCZOS)
    Path("wallpaper.ppm").write_bytes(b"P6\n1920 1080\n255\n" + image.tobytes())
```

The builder installs this asset as `/ETC/WALL.PPM`. Only the DE loads/decodes it;
the compositor receives a rendered background surface and knows no image format.
