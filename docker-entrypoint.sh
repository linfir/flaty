#!/bin/sh
set -eu

if [ "$#" -eq 0 ]; then
	set -- \
		--bind "${FLATY_BIND:-0.0.0.0}" \
		--port "${FLATY_PORT:-8080}" \
		--directory "${FLATY_DIRECTORY:-/data}"

	case "${FLATY_MULTI:-}" in
		1 | true | TRUE | yes | YES | on | ON)
			set -- "$@" --multi
			;;
	esac

	exec flaty "$@"
fi

if [ "${1#-}" != "$1" ]; then
	exec flaty "$@"
fi

exec "$@"
