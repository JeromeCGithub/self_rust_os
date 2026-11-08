"""Compare unique native CSS background bounds with real-browser measurements."""

def dark_bounds(image):
    pixels=image.convert('RGB')
    points=[(x,y) for y in range(pixels.height) for x in range(pixels.width)
            if max(pixels.getpixel((x,y)))<80]
    if not points:return None
    xs,ys=zip(*points)
    return [min(xs),min(ys),max(xs)-min(xs)+1,max(ys)-min(ys)+1]

def assert_rectangles(image, expected, inset=0):
    assert 0 <= inset < image.width // 2
    pixels = image.convert('RGB')
    for item in expected:
        color = item['color']
        rgb = (color >> 16, (color >> 8) & 255, color & 255)
        points = [(x, y) for y in range(pixels.height) for x in range(inset,pixels.width-inset)
                  if pixels.getpixel((x, y)) == rgb]
        assert points, f'missing CSS background {color:#08x}'
        xs, ys = zip(*points)
        actual = [min(xs), min(ys), max(xs) - min(xs) + 1, max(ys) - min(ys) + 1]
        x,y,width,height=item['box']
        left=max(inset,x);right=min(pixels.width-inset,x+width)
        bounds=[left,y,right-left,height]
        assert actual == bounds, f'CSS background {color:#08x}: {actual} != {bounds}'
