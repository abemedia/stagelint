#!/usr/bin/env sh
set -e

RELEASES_URL="https://github.com/abemedia/stagelint/releases"

fail() {
	echo "$1" >&2
	exit 1
}

download() {
	if command -v curl >/dev/null 2>&1; then
		curl -sSfL -o "$2" "$1"
	else
		wget -q -O "$2" "$1"
	fi || fail "Could not download $1."
}

main() {
	command -v curl >/dev/null 2>&1 || command -v wget >/dev/null 2>&1 || fail "curl or wget is required."

	OS="$(uname -s)"
	ARCH="$(uname -m)"
	test "$ARCH" = "amd64" && ARCH="x86_64"
	test "$ARCH" = "arm64" && ARCH="aarch64"

	EXT="tar.gz"
	BIN="stagelint"
	case "$OS" in
	Linux) TARGET="$ARCH-unknown-linux-musl" ;;
	Darwin)
		# An Apple Silicon Mac running a translated shell reports x86_64.
		test "$(sysctl -n sysctl.proc_translated 2>/dev/null)" = "1" && ARCH="aarch64"
		TARGET="$ARCH-apple-darwin"
		;;
	MINGW* | MSYS* | CYGWIN*)
		EXT="zip"
		BIN="stagelint.exe"
		test "$ARCH" = "aarch64" && TARGET="aarch64-pc-windows-gnullvm" || TARGET="$ARCH-pc-windows-gnu"
		;;
	*) fail "No prebuilt stagelint binary for $OS/$ARCH. Build it from source with 'cargo install stagelint'." ;;
	esac

	if test -n "$STAGELINT_VERSION"; then
		CHECKSUMS_URL="$RELEASES_URL/download/v${STAGELINT_VERSION#v}/checksums.txt"
	else
		CHECKSUMS_URL="$RELEASES_URL/latest/download/checksums.txt"
	fi

	INSTALL_DIR="$STAGELINT_INSTALL_DIR"
	test -z "$INSTALL_DIR" && test -w /usr/local/bin && INSTALL_DIR="/usr/local/bin"
	test -z "$INSTALL_DIR" && INSTALL_DIR="$HOME/.local/bin"

	TMP_DIR="$(mktemp -d)"
	# shellcheck disable=SC2064 # intentionally expands here
	trap "rm -rf \"$TMP_DIR\"" EXIT
	trap 'exit 1' INT TERM

	download "$CHECKSUMS_URL" "$TMP_DIR/checksums.txt"
	LINE="$(grep " stagelint-[^ ]*-$TARGET\.$EXT\$" "$TMP_DIR/checksums.txt")" || fail "No prebuilt stagelint binary for $OS/$ARCH. Build it from source with 'cargo install stagelint'."
	ARCHIVE="${LINE##* }"
	VERSION="${ARCHIVE#stagelint-}"
	VERSION="v${VERSION%"-$TARGET.$EXT"}"

	echo "Downloading stagelint $VERSION for $TARGET..."
	download "$RELEASES_URL/download/$VERSION/$ARCHIVE" "$TMP_DIR/$ARCHIVE"

	echo "Verifying checksums..."
	if command -v sha256sum >/dev/null 2>&1; then
		SUM="$(sha256sum "$TMP_DIR/$ARCHIVE")"
	elif command -v shasum >/dev/null 2>&1; then
		SUM="$(shasum -a 256 "$TMP_DIR/$ARCHIVE")"
	else
		fail "Could not verify checksums, no sha256 tool found."
	fi
	test "${SUM%% *}" = "${LINE%% *}" || fail "Checksum mismatch for $ARCHIVE."

	if command -v cosign >/dev/null 2>&1; then
		echo "Verifying signatures..."
		download "$RELEASES_URL/download/$VERSION/checksums.txt.sigstore.json" "$TMP_DIR/checksums.txt.sigstore.json"
		cosign verify-blob \
			--certificate-identity "https://github.com/abemedia/stagelint/.github/workflows/publish.yml@refs/tags/$VERSION" \
			--certificate-oidc-issuer 'https://token.actions.githubusercontent.com' \
			--bundle "$TMP_DIR/checksums.txt.sigstore.json" \
			"$TMP_DIR/checksums.txt"
	else
		echo "Could not verify signatures, cosign is not installed."
	fi

	(
		cd "$TMP_DIR"
		case "$EXT" in
		tar.gz) tar -xzf "$ARCHIVE" ;;
		zip)
			if command -v unzip >/dev/null 2>&1; then
				unzip -qo "$ARCHIVE"
			else
				powershell -NoProfile -Command "Expand-Archive -Force '$ARCHIVE' ."
			fi
			;;
		esac
	)

	mkdir -p "$INSTALL_DIR"
	install -m 755 "$TMP_DIR/$BIN" "$INSTALL_DIR/$BIN"

	echo "Installed stagelint $VERSION to $INSTALL_DIR"
	case ":$PATH:" in
	*":$INSTALL_DIR:"*) ;;
	*) echo "Add $INSTALL_DIR to your PATH to use it." ;;
	esac
}

main
