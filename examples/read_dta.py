#!/usr/bin/env python3
"""Example: read a Stata .dta file from Python via searchlight_cli, no Stata needed.

Usage:
    python read_dta.py path/to/file.dta

This shells out to the searchlight_cli binary and parses its JSON output the same way any program
would. Adjust EXE below if the binary is not on your PATH.
"""

import json
import subprocess
import sys

# Path to the compiled binary. If it is on your PATH, just "searchlight_cli" works.
EXE = "searchlight_cli"


def stata(dta_path, command, *flags):
    """Run one Stata command against a .dta file and return the parsed JSON (or raw text for JSONL)."""
    proc = subprocess.run(
        [EXE, dta_path, *flags, "-c", command],
        capture_output=True,
        text=True,
    )
    if proc.returncode == 2:
        # Usage / parse / command error: details are JSON on stderr.
        raise RuntimeError(json.loads(proc.stderr).get("error", proc.stderr))
    return proc


def main():
    if len(sys.argv) < 2:
        print("usage: python read_dta.py <file.dta>", file=sys.stderr)
        sys.exit(1)
    dta = sys.argv[1]

    # 1. Metadata.
    desc = json.loads(stata(dta, "describe").stdout)
    print(f"{desc['observations']} obs x {desc['variables']} vars"
          f"  ({desc['label'] or 'no label'})")

    # 2. Summary statistics.
    summ = json.loads(stata(dta, "summarize").stdout)
    print("\nvariable            mean            sd")
    for v in summ["variables"]:
        if v["mean"] is not None:
            print(f"  {v['variable']:<15} {v['mean']:>12}  {v['sd']:>12}")

    # 3. Extract the first few rows as dicts.
    rows = json.loads(stata(dta, "list in 1/5").stdout)["data"]
    print("\nfirst rows:")
    for row in rows:
        print("  ", {k: v for k, v in row.items() if k != "_n"})

    # 4. Stream all rows via JSONL (memory-friendly for large files).
    text = stata(dta, "list", "--jsonl").stdout
    total = sum(1 for line in text.splitlines() if line.strip())
    print(f"\nstreamed {total} observations via JSONL")


if __name__ == "__main__":
    main()
