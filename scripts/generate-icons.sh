#!/bin/bash
# scripts/generate-icons.sh
# Professional icon generation for all platforms

set -e

# Colors for better output
GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}🎨 Generating icons for all platforms...${NC}"

# Check if source SVG exists
if [ ! -f "assets/flash_logo.svg" ]; then
    echo "❌ Error: assets/flash_logo.svg not found!"
    exit 1
fi

# Create main icons directory
mkdir -p assets/icons

# Check ImageMagick
if ! command -v magick >/dev/null 2>&1; then
    echo "❌ ImageMagick not found. Please install:"
    echo "  macOS: brew install imagemagick"
    echo "  Linux: sudo apt-get install imagemagick"
    exit 1
fi

echo -e "${GREEN}📱 Generating macOS icons (.icns)...${NC}"
# =============================================================================
# macOS: Create .icns file from iconset
# =============================================================================
mkdir -p assets/icons/macos.iconset

# Standard sizes for macOS iconset
magick assets/flash_logo.svg -resize 16x16 assets/icons/macos.iconset/icon_16x16.png
magick assets/flash_logo.svg -resize 32x32 assets/icons/macos.iconset/icon_32x32.png
magick assets/flash_logo.svg -resize 128x128 assets/icons/macos.iconset/icon_128x128.png
magick assets/flash_logo.svg -resize 256x256 assets/icons/macos.iconset/icon_256x256.png
magick assets/flash_logo.svg -resize 512x512 assets/icons/macos.iconset/icon_512x512.png

# @2x (Retina) versions for macOS iconset
magick assets/flash_logo.svg -resize 32x32 assets/icons/macos.iconset/icon_16x16@2x.png
magick assets/flash_logo.svg -resize 64x64 assets/icons/macos.iconset/icon_32x32@2x.png
magick assets/flash_logo.svg -resize 256x256 assets/icons/macos.iconset/icon_128x128@2x.png
magick assets/flash_logo.svg -resize 512x512 assets/icons/macos.iconset/icon_256x256@2x.png
magick assets/flash_logo.svg -resize 1024x1024 assets/icons/macos.iconset/icon_512x512@2x.png

# Create .icns file (only works on macOS)
if command -v iconutil >/dev/null 2>&1; then
    iconutil -c icns assets/icons/macos.iconset -o assets/icons/icon.icns
    echo "✅ Created icon.icns for macOS"
else
    echo "⚠️  iconutil not available (run this on macOS to generate .icns)"
fi

# Clean up iconset directory (we only need the .icns file)
rm -rf assets/icons/macos.iconset

echo -e "${GREEN}🪟 Generating Windows icons (.ico)...${NC}"
# =============================================================================
# Windows: Create .ico file with multiple embedded sizes
# =============================================================================
# Create temporary directory for Windows icon generation
mkdir -p assets/icons/temp_windows

# Generate individual PNGs for Windows ICO
for size in 16 32 48 64 128 256; do
    magick assets/flash_logo.svg -resize ${size}x${size} assets/icons/temp_windows/icon-${size}.png
done

# Combine all sizes into one ICO file
magick assets/icons/temp_windows/icon-*.png assets/icons/icon.ico

# Clean up temporary files
rm -rf assets/icons/temp_windows
echo "✅ Created icon.ico for Windows"

echo -e "${GREEN}🐧 Generating Linux icons (.png)...${NC}"
# =============================================================================
# Linux: Individual PNG files for different contexts
# =============================================================================
# Standard Linux icon sizes (FreeDesktop specification)
for size in 16 24 32 48 64 128 256 512; do
    magick assets/flash_logo.svg -resize ${size}x${size} assets/icons/linux-icon-${size}.png
done
echo "✅ Created PNG icons for Linux"

# =============================================================================
# Summary
# =============================================================================
echo ""
echo -e "${BLUE}🎉 Icon generation complete!${NC}"
echo ""
echo "Generated files:"
echo "  📱 assets/icons/icon.icns           → macOS app bundle"
echo "  🪟 assets/icons/icon.ico            → Windows executable"
echo "  🐧 assets/icons/linux-icon-*.png    → Linux desktop files"
echo ""
echo "File structure:"
tree assets/icons/ 2>/dev/null || ls -la assets/icons/
echo ""
echo "Next steps:"
echo "  1. Update Cargo.toml to use: icon = [\"assets/icons/icon.icns\"]"
echo "  2. The build system will automatically use the right icons per platform"
echo "  3. Ready for release! 🚀"