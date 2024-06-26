#!/bin/sh

cd "`dirname $0`/.."

TIMESTAMP=`date --utc '+%+4Y-%m-%d-%H%M'`

CTRLS="$*"
if [ X"$CTRLS" = X ]; then
    CTRLS="declctrl pyctrl jsctrl guidance_ctrl"
fi

for ctrl in $CTRLS ; do
    ./aici.sh build controllers/$ctrl -T $ctrl-latest -T $ctrl-$TIMESTAMP
done
