#!/bin/sh
# Single live-session lock shared by all agents (one PRK account, one session).
#   scripts/live-lock.sh acquire <name>   -> exit 0 when acquired (waits up to 15 min), 1 on timeout
#   scripts/live-lock.sh release <name>   -> removes the lock only if <name> owns it
#   scripts/live-lock.sh status
# A lock older than 10 minutes is stale and may be taken over.
L=/tmp/aomac-live.lock
cmd=$1 name=$2
case $cmd in
acquire)
	[ -n "$name" ] || { echo "usage: $0 acquire <name>" >&2; exit 2; }
	i=0
	while [ $i -lt 180 ]; do
		if mkdir "$L" 2>/dev/null; then
			echo "$name $(date +%s)" >"$L/owner"
			exit 0
		fi
		age=$(( $(date +%s) - $(stat -f %m "$L" 2>/dev/null || date +%s) ))
		if [ "$age" -gt 600 ]; then
			echo "stale lock ($(cat "$L/owner" 2>/dev/null)), taking over" >&2
			rm -rf "$L"
			continue
		fi
		sleep 5
		i=$((i + 1))
	done
	echo "timeout; held by $(cat "$L/owner" 2>/dev/null)" >&2
	exit 1 ;;
release)
	owner=$(cut -d' ' -f1 "$L/owner" 2>/dev/null)
	if [ "$owner" = "$name" ]; then rm -rf "$L"; else echo "not owner (held by '$owner')" >&2; exit 1; fi ;;
status)
	cat "$L/owner" 2>/dev/null || echo free ;;
*)
	echo "usage: $0 acquire|release <name> | status" >&2; exit 2 ;;
esac
