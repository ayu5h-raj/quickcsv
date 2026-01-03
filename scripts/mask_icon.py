import sys
from PIL import Image, ImageDraw


def add_corners(im, rad):
    # Convert to RGBA
    im = im.convert("RGBA")
    
    # Floodfill from corners to remove background (white or otherwise)
    # We sample the color at (0,0) and make it transparent
    bg_color = im.getpixel((0, 0))
    # Tolerance for floodfill (simple exact match or small threshold)
    # PIL floodfill requires exact match or sample. We use a simple diff approach?
    # actually ImageDraw.floodfill works well.
    
    # Create valid mask image for floodfill? No, floodfill modifies image in place.
    # We floodfill with (0,0,0,0)
    ImageDraw.floodfill(im, (0, 0), (0, 0, 0, 0), thresh=50)
    # Also other corners just in case connectivity is broken (unlikely for a box)
    w, h = im.size
    ImageDraw.floodfill(im, (w-1, 0), (0, 0, 0, 0), thresh=50)
    ImageDraw.floodfill(im, (0, h-1), (0, 0, 0, 0), thresh=50)
    ImageDraw.floodfill(im, (w-1, h-1), (0, 0, 0, 0), thresh=50)

    # Now apply the soft rounded mask to ensure smooth edges
    circle = Image.new('L', (rad * 2, rad * 2), 0)
    draw = ImageDraw.Draw(circle)
    draw.ellipse((0, 0, rad * 2, rad * 2), fill=255)
    
    # Existing alpha
    alpha = im.getchannel('A')
    
    # Create mask for corners
    mask = Image.new('L', im.size, 255)
    mask.paste(circle.crop((0, 0, rad, rad)), (0, 0))
    mask.paste(circle.crop((0, rad, rad, rad * 2)), (0, h - rad))
    mask.paste(circle.crop((rad, 0, rad * 2, rad)), (w - rad, 0))
    mask.paste(circle.crop((rad, rad, rad * 2, rad * 2)), (w - rad, h - rad))
    
    # Combine floodfilled alpha with rounded mask (minimum of both)
    # This keeps the floodfilled transparency AND cuts off anything sticking out
    combined_alpha = Image.composite(Image.new('L', im.size, 0), alpha, Image.eval(mask, lambda a: 255-a))
    # Wait, simple multiplication or min?
    # Let's just create a new alpha by pasting the mask on top of *existing* alpha?
    # Cleaner: new_alpha = min(alpha, mask)
    # PIL Chops darker/multiply
    from PIL import ImageChops
    final_alpha = ImageChops.multiply(alpha, mask)
    
    im.putalpha(final_alpha)
    return im

if __name__ == "__main__":
    if len(sys.argv) != 3:
        print("Usage: python mask_icon.py <input_path> <output_path>")
        sys.exit(1)

    input_path = sys.argv[1]
    output_path = sys.argv[2]

    try:
        img = Image.open(input_path).convert("RGBA")
        
        # Calculate radius (approx 17.5% of size for macOS style)
        radius = int(min(img.size) * 0.175)
        
        # Determine strict mask (removes white background if it exists in corners)
        # Actually, if the image is full square with the design, we just mask it.
        # If the generated image already has rounded corners but black/white background, 
        # masking it will cut that off.
        
        img = add_corners(img, radius)
        img.save(output_path, "PNG")
        print(f"Saved masked image to {output_path}")
        
    except Exception as e:
        print(f"Error: {e}")
        sys.exit(1)
