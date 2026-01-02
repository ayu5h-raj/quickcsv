#!/bin/bash
# Build script for QuickCSV macOS distribution

set -e

echo "🔨 Building QuickCSV for macOS..."

# Build release binary
cargo build --release

# Create macOS app bundle
echo "📦 Creating macOS app bundle..."
cargo bundle --release

# Find the created bundle
BUNDLE_PATH="target/release/bundle/osx/QuickCSV.app"

if [ -d "$BUNDLE_PATH" ]; then
    echo "✅ App bundle created at: $BUNDLE_PATH"
    
    # Optional: Create a DMG
    if command -v create-dmg &> /dev/null; then
        echo "💿 Creating DMG installer..."
        create-dmg \
            --volname "QuickCSV" \
            --window-pos 200 120 \
            --window-size 600 400 \
            --icon-size 100 \
            --icon "QuickCSV.app" 150 185 \
            --app-drop-link 450 185 \
            "QuickCSV-$(cargo metadata --format-version 1 | jq -r '.packages[] | select(.name=="quickcsv") | .version').dmg" \
            "$BUNDLE_PATH"
    else
        echo "💡 Tip: Install create-dmg for DMG creation: brew install create-dmg"
    fi
    
    # Show bundle info
    echo ""
    echo "📋 Bundle info:"
    ls -la "$BUNDLE_PATH"
    echo ""
    echo "📁 Bundle contents:"
    ls -la "$BUNDLE_PATH/Contents/"
else
    echo "❌ Bundle creation failed"
    exit 1
fi

echo ""
echo "🎉 Done! You can now:"
echo "   1. Run the app: open '$BUNDLE_PATH'"
echo "   2. Copy to Applications: cp -r '$BUNDLE_PATH' /Applications/"
echo "   3. Create a ZIP for distribution: zip -r QuickCSV.zip '$BUNDLE_PATH'"
