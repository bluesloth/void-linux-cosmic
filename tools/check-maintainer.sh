#!/bin/sh
# check-maintainer.sh - identity gate for this repository.
#
# Every template must declare the canonical maintainer identity below, so the
# packaging sources and the built packages advertise the same maintainer.
#
# An optional external denylist can also reject additional identifiers. It is
# kept out of the repository (see .gitignore): set MAINTAINER_DENYLIST to its
# path, or place it at ./.maintainer-denylist (one extended regex per line;
# '#' comments). A CI-style run without a denylist simply skips that pass.
set -u

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
canon='Void Cosmic <noreply@github.com>'
status=0

# 1) every template must carry exactly the canonical maintainer line
n=0
bad=""
for t in "$root"/srcpkgs/*/template; do
	n=$((n + 1))
	if ! grep -qxF "maintainer=\"$canon\"" "$t"; then
		bad="$bad $t"
	fi
done
if [ -n "$bad" ]; then
	printf 'FAIL: templates without the canonical maintainer:%s\n' "$bad" >&2
	status=1
fi

# 2) optional external denylist of extra disallowed identifiers
deny="${MAINTAINER_DENYLIST:-$root/.maintainer-denylist}"
if [ -f "$deny" ]; then
	tmp=$(mktemp) || exit 1
	grep -v '^[[:space:]]*#' "$deny" | grep -v '^[[:space:]]*$' > "$tmp" || true
	if [ -s "$tmp" ]; then
		hits=$(grep -rInE --exclude=check-maintainer.sh -f "$tmp" \
			"$root/srcpkgs" "$root/common" "$root/tools" "$root/docs" \
			"$root/README.md" "$root/PINS.md" 2>/dev/null || true)
		if [ -n "$hits" ]; then
			printf 'FAIL: disallowed identifier present:\n%s\n' "$hits" >&2
			status=1
		fi
	fi
	rm -f "$tmp"
fi

if [ "$status" -eq 0 ]; then
	printf 'OK: canonical maintainer in all %s templates\n' "$n"
fi
exit "$status"
