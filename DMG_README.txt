Claude-o-Meter
==============

A lightweight macOS menu bar app that shows your Claude Code plan usage at a
glance. Displays a color-coded ring gauge in the menu bar showing your current
usage window. Click it for a full breakdown of all windows with progress bars,
reset countdowns, and usage alerts.

Prerequisites
-------------
You must have an active Claude Code session. If you haven't already, run:

    claude login

This stores OAuth credentials in your macOS Keychain that Claude-o-Meter reads
to fetch your usage data. No credentials are stored or transmitted by this app.

Installation
------------
Double-click "Claude-o-Meter.pkg" to launch the installer. It will:

  1. Install Claude-o-Meter to /Applications
  2. Configure it to start automatically at login
  3. Launch the app when installation completes

The gauge icon will appear in your menu bar. Click it to see usage details and
configure settings like refresh interval and alert threshold.

Uninstall
---------
Drag Claude-o-Meter from /Applications to the Trash. The app automatically
detects the removal, cleans up its Launch Agent, and quits.
