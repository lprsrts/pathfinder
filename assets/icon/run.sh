#!/bin/sh
# usage: run.sh '<python dict for V>'
cd "$(dirname "$0")"
"${BLENDER:-/Applications/Blender.app/Contents/MacOS/Blender}" -b _base.blend --python-expr "V = $1
exec(open('build5.py').read())
print('RESULT', result)" 2>&1 | grep -E "RESULT|Error|Traceback|line [0-9]+|Saved|Time:" | tail -15
