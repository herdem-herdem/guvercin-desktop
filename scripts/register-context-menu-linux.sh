#!/bin/bash
# Register context menu for Linux (GNOME, KDE, etc.)
# Run with: bash register-context-menu-linux.sh

DESKTOP_ACTION_DIR="$HOME/.local/share/nautilus/scripts"
DESKTOP_ACTION_DIR_KDE="$HOME/.local/share/kio/servicemenus"
DESKTOP_FILE_DIR="$HOME/.local/share/applications"
DESKTOP_FILE="$DESKTOP_FILE_DIR/guvercin-attach.desktop"

# Create directories
mkdir -p "$DESKTOP_ACTION_DIR"
mkdir -p "$DESKTOP_ACTION_DIR_KDE"
mkdir -p "$DESKTOP_FILE_DIR"

# Check if Guvercin is installed
if ! command -v guvercin &> /dev/null && [ ! -f "/usr/bin/guvercin" ] && [ ! -f "/usr/local/bin/guvercin" ]; then
    echo "Warning: guvercin executable not found in PATH"
    echo "Make sure Guvercin is properly installed."
fi

# Create Nautilus (GNOME Files) script
cat > "$DESKTOP_ACTION_DIR/Send with Guvercin" << 'EOF'
#!/bin/bash
# Nautilus script to send file with Guvercin
for file in $NAUTILUS_SCRIPT_SELECTED_FILE_PATHS; do
    encoded_path=$(printf %s "$file" | sed 's/ /%20/g;s/&/%26/g;s/?/%3F/g')
    xdg-open "guvercin://attach-file?path=$encoded_path" &
done
EOF

chmod +x "$DESKTOP_ACTION_DIR/Send with Guvercin"

# Create KDE Dolphin service menu
cat > "$DESKTOP_ACTION_DIR_KDE/guvercin-attach.desktop" << 'EOF'
[Desktop Entry]
Type=Service
ServiceTypes=KonqPopupMenu/Plugin
MimeTypes=all/all
Actions=SendWithGuvercin

[Desktop Action SendWithGuvercin]
Name=Send with Guvercin
Exec=sh -c 'xdg-open "guvercin://attach-file?path=%f"'
Icon=mail
EOF

# Create .desktop file for app itself (if not exists)
if [ ! -f "$DESKTOP_FILE" ]; then
    cat > "$DESKTOP_FILE" << 'EOF'
[Desktop Entry]
Type=Application
Name=Guvercin
Comment=Email client
Exec=guvercin %f
MimeType=message/rfc822;x-scheme-handler/mailto;x-scheme-handler/guvercin;
Categories=Office;Email;
Icon=mail
StartupNotify=true
EOF
    chmod +x "$DESKTOP_FILE"
fi

echo "✓ Linux context menu registered successfully!"
echo ""
echo "The context menu should now appear in:"
echo "  • GNOME Files (Nautilus)"
echo "  • KDE Dolphin"
echo "  • Other file managers that support these standards"
echo ""
echo "You may need to restart your file manager for changes to take effect."
