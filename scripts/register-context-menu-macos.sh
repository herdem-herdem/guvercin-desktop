#!/bin/bash
# Register a macOS "Send with Guvercin" Quick Action (Services menu item).
# Run with: bash register-context-menu-macos.sh
#
# This builds a real Automator Quick Action bundle. A valid .workflow MUST
# contain Contents/document.wflow (the Automator document) alongside
# Contents/Info.plist. A workflow is NOT an app bundle, so it has no
# CFBundleExecutable / Contents/MacOS — that was the bug that made macOS
# report "damaged or incomplete".

set -euo pipefail

SERVICES_DIR="$HOME/Library/Services"
WORKFLOW_DIR="$SERVICES_DIR/Send with Guvercin.workflow"
CONTENTS_DIR="$WORKFLOW_DIR/Contents"

# Verify the app is installed (either casing).
if [ ! -d "/Applications/guvercin.app" ] && [ ! -d "/Applications/Guvercin.app" ]; then
    echo "Error: Guvercin.app not found in /Applications" >&2
    exit 1
fi

# Start clean so we never leave a half-written (damaged) bundle behind.
rm -rf "$WORKFLOW_DIR"
mkdir -p "$CONTENTS_DIR"

# --- Info.plist: declares the Service menu item ---------------------------
cat > "$CONTENTS_DIR/Info.plist" << 'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>NSServices</key>
	<array>
		<dict>
			<key>NSMenuItem</key>
			<dict>
				<key>default</key>
				<string>Send with Guvercin</string>
			</dict>
			<key>NSMessage</key>
			<string>runWorkflowAsService</string>
			<key>NSRequiredContext</key>
			<dict>
				<key>NSApplicationIdentifier</key>
				<string>com.apple.finder</string>
			</dict>
			<key>NSSendFileTypes</key>
			<array>
				<string>public.item</string>
			</array>
		</dict>
	</array>
</dict>
</plist>
EOF

# --- The shell command the Quick Action runs ------------------------------
# Input is passed "as arguments", so selected file paths arrive in "$@".
# We percent-encode each path in pure bash (no jq/python dependency) and hand
# it to Guvercin via the custom URI scheme.
read -r -d '' SHELL_COMMAND << 'EOF' || true
# Percent-encode a path byte-by-byte with perl (always present on macOS).
# perl reads the string as raw bytes, so multibyte UTF-8 characters — e.g. the
# U+202F narrow no-break space macOS puts in screenshot filenames — are encoded
# as valid %XX%XX%XX sequences that decodeURIComponent can decode. A pure-bash
# loop mis-encodes these (sign-extended %FFFF..E2), which broke parsing.
for f in "$@"; do
    encoded=$(printf '%s' "$f" | perl -pe 's/([^A-Za-z0-9._~\/-])/sprintf("%%%02X", ord($1))/ge')
    open "guvercin://attach-file?path=$encoded"
done
EOF

# XML-escape the command so it can be embedded in the plist string.
escape_xml() {
    local s="$1"
    s="${s//&/&amp;}"
    s="${s//</&lt;}"
    s="${s//>/&gt;}"
    printf '%s' "$s"
}
ESCAPED_COMMAND="$(escape_xml "$SHELL_COMMAND")"

# --- document.wflow: the Automator document (Run Shell Script action) ------
cat > "$CONTENTS_DIR/document.wflow" << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>AMApplicationBuild</key>
	<string>521</string>
	<key>AMApplicationVersion</key>
	<string>2.10</string>
	<key>AMDocumentVersion</key>
	<string>2</string>
	<key>actions</key>
	<array>
		<dict>
			<key>action</key>
			<dict>
				<key>AMAccepts</key>
				<dict>
					<key>Container</key>
					<string>List</string>
					<key>Optional</key>
					<true/>
					<key>Types</key>
					<array>
						<string>com.apple.cocoa.string</string>
					</array>
				</dict>
				<key>AMActionVersion</key>
				<string>2.0.3</string>
				<key>AMApplication</key>
				<array>
					<string>Automator</string>
				</array>
				<key>AMParameterProperties</key>
				<dict>
					<key>COMMAND_STRING</key>
					<dict/>
					<key>CheckedForUserDefaultShell</key>
					<dict/>
					<key>inputMethod</key>
					<dict/>
					<key>shell</key>
					<dict/>
					<key>source</key>
					<dict/>
				</dict>
				<key>AMProvides</key>
				<dict>
					<key>Container</key>
					<string>List</string>
					<key>Types</key>
					<array>
						<string>com.apple.cocoa.string</string>
					</array>
				</dict>
				<key>ActionBundlePath</key>
				<string>/System/Library/Automator/Run Shell Script.action</string>
				<key>ActionName</key>
				<string>Run Shell Script</string>
				<key>ActionParameters</key>
				<dict>
					<key>COMMAND_STRING</key>
					<string>${ESCAPED_COMMAND}</string>
					<key>CheckedForUserDefaultShell</key>
					<true/>
					<key>inputMethod</key>
					<integer>1</integer>
					<key>shell</key>
					<string>/bin/bash</string>
					<key>source</key>
					<string></string>
				</dict>
				<key>BundleIdentifier</key>
				<string>com.apple.RunShellScript</string>
				<key>CFBundleVersion</key>
				<string>2.0.3</string>
				<key>CanShowSelectedItemsWhenRun</key>
				<false/>
				<key>CanShowWhenRun</key>
				<true/>
				<key>Category</key>
				<array>
					<string>AMCategoryUtilities</string>
				</array>
				<key>Class Name</key>
				<string>RunShellScriptAction</string>
				<key>InputUUID</key>
				<string>00000000-0000-0000-0000-000000000001</string>
				<key>Keywords</key>
				<array>
					<string>Shell</string>
					<string>Script</string>
					<string>Command</string>
					<string>Run</string>
					<string>Unix</string>
				</array>
				<key>OutputUUID</key>
				<string>00000000-0000-0000-0000-000000000002</string>
				<key>UUID</key>
				<string>00000000-0000-0000-0000-000000000003</string>
				<key>arguments</key>
				<dict/>
				<key>isViewVisible</key>
				<integer>1</integer>
			</dict>
			<key>isViewVisible</key>
			<integer>1</integer>
		</dict>
	</array>
	<key>connectors</key>
	<dict/>
	<key>workflowMetaData</key>
	<dict>
		<key>serviceInputTypeIdentifier</key>
		<string>com.apple.Automator.fileSystemObject</string>
		<key>serviceOutputTypeIdentifier</key>
		<string>com.apple.Automator.nothing</string>
		<key>serviceProcessesInput</key>
		<integer>0</integer>
		<key>workflowTypeIdentifier</key>
		<string>com.apple.Automator.servicesMenu</string>
	</dict>
</dict>
</plist>
EOF

# Ask the system to re-scan Services so the item appears without a relogin.
/System/Library/CoreServices/pbs -flush 2>/dev/null || true

echo "✓ macOS Quick Action 'Send with Guvercin' registered successfully!"
echo "Right-click a file in Finder → Quick Actions (or Services) → 'Send with Guvercin'."
echo "If it doesn't appear immediately, log out and back in, or check"
echo "System Settings → Keyboard → Keyboard Shortcuts → Services."
