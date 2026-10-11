#!/bin/sh
# sync-upstream.sh - keep these COSMIC templates in step with upstream.
#
# Upstream of record: https://github.com/pop-os/cosmic-epoch
#
#   COSMIC ships as "epoch" releases. The aggregate repository (cosmic-epoch)
#   and every component repository (pop-os/<component>) carry the same
#   `epoch-X.Y.Z` tag, and our templates fetch
#       https://github.com/pop-os/<component>/archive/refs/tags/epoch-<ver>.tar.gz
#
# Usage:
#   sync-upstream.sh              --check (default): report drift; exit 2 if behind
#   sync-upstream.sh --list       --show pinned vs upstream epoch
#   sync-upstream.sh --apply      --pin templates to the latest upstream epoch
#   sync-upstream.sh --apply --to X.Y.Z
#   sync-upstream.sh --pins       --regenerate PINS.md from the templates (no network)
#
# --apply rewrites every template that tracks the epoch:
#   version=     only where it followed the previous epoch (pop-launcher keeps 1.2.7)
#   distfiles=   any literal epoch-X.Y.Z token is repinned
#   checksum=    recomputed from the downloaded tarball
#   revision=    reset to 1 when the file changed
#
# libcosmic is pinned to a commit (not an epoch tag) and is never rewritten -
# advance it by hand; see docs/UPSTREAM.md.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
pkgs=$root/srcpkgs
cache=$root/distfiles
pins=$root/PINS.md
epoch_repo=https://github.com/pop-os/cosmic-epoch.git

mode=check; target=
while [ $# -gt 0 ]; do
	case $1 in
		--check) mode=check ;;
		--list)  mode=list ;;
		--apply) mode=apply ;;
		--to)    shift; target=${1:?--to requires X.Y.Z}; mode=apply ;;
		--pins)  mode=pins ;;
		-h|--help) sed -n '2,26p' "$0"; exit 0 ;;
		*) printf 'sync-upstream: unknown argument: %s\n' "$1" >&2; exit 64 ;;
	esac
	shift
done

field() { # <template> <key>
	sed -n "s/^$2=//p" "$1" | head -n1 | sed 's/^"//; s/"$//'
}

latest_epoch() {
	git ls-remote --tags "$epoch_repo" 'refs/tags/epoch-*' 2>/dev/null |
		sed 's#.*refs/tags/epoch-##' | grep -v '\^{}' |
		grep -E '^[0-9]+(\.[0-9]+)+$' | sort -V | tail -n1
}

baseline() { # the epoch our templates are pinned to (cosmic-session is canonical)
	field "$pkgs/cosmic-session/template" version
}

fetch_sum() { # <pkg> <url-resolved> <epoch>
	out=$cache/$1-epoch-$3.tar.gz
	mkdir -p "$cache"
	[ -f "$out" ] || curl -fL --retry 3 --retry-delay 2 -o "$out" "$2"
	sha256sum "$out" | awk '{print $1}'
}

write_pins() {
	ce=$(baseline)
	{
		printf '# PINS - Void_Linux_Cosmic\n\n'
		printf 'Upstream of record: [`pop-os/cosmic-epoch`](https://github.com/pop-os/cosmic-epoch)\n'
		printf 'Tracked epoch: `%s` (`epoch-%s`)\n\n' "$ce" "$ce"
		printf '| package | version | checksum |\n| --- | --- | --- |\n'
		for t in "$pkgs"/*/template; do
			n=$(basename "$(dirname "$t")")
			printf '| %s | %s | %s |\n' "$n" "$(field "$t" version)" "$(field "$t" checksum)"
		done
	} > "$pins"
	printf 'wrote %s (epoch %s)\n' "$pins" "$ce"
}

if [ "$mode" = pins ]; then
	write_pins
	exit 0
fi

cur=$(baseline)
if [ -n "$target" ]; then
	latest=$target
else
	latest=$(latest_epoch)
	[ -n "$latest" ] || { echo "sync-upstream: cannot reach $epoch_repo" >&2; exit 1; }
fi
target=$latest

if [ "$mode" = list ]; then
	printf 'pinned epoch : %s\n' "$cur"
	printf 'upstream     : %s\n' "$latest"
	exit 0
fi

drift=0; hdr=0; changed=0
for t in "$pkgs"/*/template; do
	[ -f "$t" ] || continue
	n=$(basename "$(dirname "$t")")
	v=$(field "$t" version)
	df=$(field "$t" distfiles)
	NEWV=; NEWDF=; NEWSUM=; why=
	if [ -z "$df" ]; then
		# aggregate / meta package: nothing to fetch, version simply tracks the epoch
		if [ "$v" != "$target" ] && [ "$v" = "$cur" ]; then
			NEWV=$target; why="meta -> $target"
		else
			why=current
		fi
	elif printf '%s' "$df" | grep -q 'epoch-'; then
		dfe=$(printf '%s' "$df" | sed -n 's#.*epoch-\([0-9][0-9.]*\)\.tar\.gz.*#\1#p')
		if [ -n "$dfe" ]; then
			# literal epoch token (e.g. pop-launcher)
			if [ "$dfe" != "$target" ]; then
				NEWDF=$(printf '%s' "$df" | sed "s#epoch-$dfe\.tar\.gz#epoch-$target.tar.gz#")
				NEWSUM=$(fetch_sum "$n" "$NEWDF" "$target")
				why="epoch $dfe -> $target"
			else
				why=current
			fi
			if [ "$v" = "$cur" ] && [ "$v" != "$target" ]; then NEWV=$target; fi
		else
			# epoch-${version}: the version drives the URL, so bump version + checksum
			if [ "$v" != "$target" ] && [ "$v" = "$cur" ]; then
				NEWV=$target
				resolved=$(printf '%s' "$df" | sed "s#\${version}#$target#g")
				NEWSUM=$(fetch_sum "$n" "$resolved" "$target")
				why="epoch $v -> $target"
			elif [ "$v" != "$target" ]; then
				why="own version $v (epoch token via distfiles)"
			else
				why=current
			fi
		fi
	elif printf '%s' "$df" | grep -qE 'archive/[0-9a-f]{40}\.tar\.gz'; then
		why="commit-pinned (manual; see docs/UPSTREAM.md)"
	else
		why="source not epoch-tagged"
	fi

	if [ -n "$NEWV$NEWDF$NEWSUM" ]; then
		drift=1
		hdr=$((hdr + 1))
		[ "$hdr" = 1 ] && printf '%-30s %-10s %s\n' package version status
		printf '  %-30s %-8s %s\n' "$n" "$v" "$why"
		if [ "$mode" = apply ]; then
			[ -n "$NEWV" ]  && sed -i "s/^version=.*/version=$NEWV/" "$t"
			[ -n "$NEWDF" ] && sed -i "s#^distfiles=.*#distfiles=\"$NEWDF\"#" "$t"
			[ -n "$NEWSUM" ] && sed -i "s/^checksum=.*/checksum=$NEWSUM/" "$t"
			sed -i "s/^revision=.*/revision=1/" "$t"
			changed=1
		fi
	fi
done

if [ "$mode" = apply ]; then
	write_pins
	if [ "$changed" = 1 ]; then
		echo "pinned to epoch $target"
		echo "note: libcosmic is commit-pinned; advance it by hand (docs/UPSTREAM.md)"
	else
		echo "already at epoch $target; nothing to change"
	fi
	exit 0
fi

if [ "$drift" = 1 ]; then
	echo "drift: templates are behind upstream epoch $target" >&2
	exit 2
fi
echo "up to date with upstream epoch $target"
