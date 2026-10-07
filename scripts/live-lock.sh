#!/bin/sh
# Single live-session lock shared by all agents (one PRK account, one session).
#   scripts/live-lock.sh acquire <name>   -> exit 0 when acquired (waits up to 15 min), 1 on timeout
#   scripts/live-lock.sh release <name>   -> removes the lock only if <name> owns it
#   scripts/live-lock.sh status
# A lock older than 10 minutes is stale and may be taken over.
# Only one agent client may run at a time: acquire waits until no agent-built
# aomac/live_walk process is alive, and release refuses while one still is.
# (Agent binaries live under /tmp/<agent>/ or this repo's target/; the user's
# own client elsewhere is never counted.)
L=/tmp/aomac-live.lock
cmd=$1 name=$2
repo=$(cd "$(dirname "$0")/.." && pwd)
clients() {
	ps -axo pid=,command= | grep -E "(^| )(/private)?(/tmp/|$repo/target/)[^ ]*/(aomac play|aomac view|live_walk)" | grep -v grep
}
case $cmd in
acquire)
	[ -n "$name" ] || { echo "usage: $0 acquire <name>" >&2; exit 2; }
	i=0
	while [ $i -lt 180 ]; do
		if [ -z "$(clients)" ] && mkdir "$L" 2>/dev/null; then
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
	[ "$owner" = "$name" ] || { echo "not owner (held by '$owner')" >&2; exit 1; }
	live=$(clients)
	if [ -n "$live" ]; then
		echo "refusing release: a client is still running; quit it and wait for it to exit:" >&2
		echo "$live" >&2
		exit 1
	fi
	rm -rf "$L" ;;
status)
	cat "$L/owner" 2>/dev/null || echo free ;;
*)
	echo "usage: $0 acquire|release <name> | status" >&2; exit 2 ;;
esac
