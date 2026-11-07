"""Opaque native surface/display readback comparison, not a security digest."""

def rgb_digest(pixels):
    digest = 14695981039346656037
    for red, green, blue in pixels:
        digest = ((digest ^ ((red << 16) | (green << 8) | blue)) * 1099511628211) & ((1 << 64) - 1)
    return digest
