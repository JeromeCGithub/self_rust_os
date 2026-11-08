# Native browser font assets

Unmodified DejaVu 2.37 Sans, Serif and Sans Mono regular/bold TrueType faces,
from the host's `ttf-dejavu` package. The font version was checked with `fc-scan`
(155320 in 16.16 format). These committed bytes make builds independent of the
host's installed fonts. Upstream: [DejaVu fonts](https://dejavu-fonts.github.io/).
Keep [the original license notices](LICENSE) with redistributed fonts.

Fontdue is pinned to 0.9.3 with its no_std hashbrown backend and SIMD/parallel/std
features disabled. Its TTF parser resolves to 0.21.1 in the committed lockfiles;
no compiler or target upgrade was made. Selected fontdue/ttf-parser notices are
in [font-notices](../font-notices/README.md). Review upstream changes and the pinned compiler
before updating. These are embedded application assets; untrusted downloaded web
fonts, shaping, bidi and broader fallback are separate implementation work.
