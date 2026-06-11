#!/bin/sh
python3 -m http.server "$PORT" --bind 127.0.0.1 &
wait
