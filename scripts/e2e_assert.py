#!/usr/bin/env python3
"""Strict JSON contracts and bounded subprocess execution for the Docker E2E."""
import json
import subprocess
import sys


def check(kind, data, args):
    def require(value):
        if not value:
            raise ValueError("contract mismatch")
    if kind == "peers":
        require(isinstance(data, list) and len(args) > 0 and len(args) % 2 == 0)
        for name, path in zip(args[::2], args[1::2]):
            matches = [p for p in data if p.get("name") == name]
            require(len(matches) == 1 and matches[0].get("path") == path)
    elif kind == "probe":
        name, path, outcome = args
        require(data.get("peer") == name and data.get("port") == 8421)
        require(data.get("path") == path)
        require(data.get("connected") is (outcome == "connected"))
        require(data.get("error") == (None if outcome == "connected" else "timeout"))
    elif kind == "status":
        require(data.get("node_name") == args[0])
        require(data.get("control_connected") is True and data.get("relay_connected") is True)
    elif kind == "acl":
        require(len(data["compiled"]["rules"]) > 0)
    else:
        raise ValueError("unknown contract")


def main():
    if sys.argv[1] in ("run", "logs"):
        # Never print argv or raw stderr: either can contain credentials.
        try:
            result = subprocess.run(sys.argv[3:], timeout=float(sys.argv[2]),
                                    stdout=subprocess.PIPE,
                                    stderr=(subprocess.STDOUT if sys.argv[1] == "logs" else subprocess.PIPE))
        except subprocess.TimeoutExpired:
            print("bounded command timed out", file=sys.stderr)
            return 124
        if result.returncode:
            print("bounded command failed", file=sys.stderr)
            return 1
        sys.stdout.buffer.write(result.stdout)
        return 0
    try:
        check(sys.argv[1], json.load(sys.stdin), sys.argv[2:])
    except (ValueError, KeyError, TypeError, AttributeError):
        print("JSON assertion failed: " + sys.argv[1], file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
