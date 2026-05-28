#!/bin/bash
# Workaround for GCC 15 windres preprocessing failure on RC files
# GCC 15's preprocessor rejects non-C tokens in RC files
# RC files from tauri-winres don't need C preprocessing, so cat suffices
exec windres --preprocessor=cat "$@"
