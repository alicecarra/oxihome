# Oxihome docs

Everything needed to build, run and operate the house.

## Getting started

- [Setup](setup.md): the development environment: ESP32 toolchain, editor, serial port access and a known linker issue.
- [MQTT broker](broker.md): Mosquitto in Docker, users and passwords, the ACL, watching raw messages.
- [Production](production.md): the stack on docker-host: first install, images, deploy, HTTPS, working on the hub.

## Firmware

- [Build and flash](firmware/build-and-flash.md): generating the project, building, flashing and `cfg.toml`.
- [Over-the-air updates](firmware/ota.md): per-board images, the hub's update server, rollback and update history.
- [Boards](firmware/boards.md): wiring and entities of the test board, the second board and the ESP32-C6 board.
- [Sensors](firmware/sensors.md): the ADXL345 accelerometer, reed switch and GPS drivers.
- [ESP-NOW leaves and gateway](firmware/espnow.md): boards that stay off the Wi-Fi and reach the broker through a gateway.

## Hub

- [Running the hub](hub/running.md): starting it, allowed hosts, login, health check and shutdown.
- [Database](hub/database.md): what SQLite keeps, retention and how to look inside.
- [Configuration](hub/configuration.md): the TOML files, validation, `nodes.toml`, rooms and `remotes.toml`.
- [Rules](hub/rules.md): `rules.toml`: triggers, conditions, actions and placeholders.
- [Rule editor](hub/rule-editor.md): the form that creates and edits rules without writing TOML.
- [Dashboard](hub/dashboard.md): `dashboard.toml`: blocks and tabs.
- [Web interface](hub/web-interface.md): the node page, htmx and events, Tailwind, Topcoat UI components.

## Nodes

- [Node library](nodes/library.md): `oxihome-node`: the `Device` trait, software nodes, protocol versions and notices.
- [Weather](nodes/weather.md): the weather outside, from Open-Meteo.
- [Linux monitor](nodes/linux-monitor.md): the state of a Linux machine and its containers.
- [Network devices](nodes/network-devices.md): which devices are on the home network, and who is home.
- [Telegram bot](nodes/telegram.md): messages from rules and commands from the chat.
- [RSS](nodes/rss.md): RSS, Atom and JSON feeds, and the YouTube instance.
- [Reminders](nodes/reminders.md): reminders with repeats, quiet hours and snooze.
- [Printer](nodes/printer.md): the state of a Creality K1 family 3D printer.
- [Simulator](nodes/simulator.md): simulated nodes for working on the hub without hardware.
