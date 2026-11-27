#!/bin/sh
# justOS starts no services of Debian's packages: their kernel does what
# they would (invoke-rc.d asks this first; 101: not allowed).
exit 101
