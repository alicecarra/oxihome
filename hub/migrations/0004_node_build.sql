-- `Hello` carries a build (name, version, target) instead of `fw_version` since protocol version 2.
ALTER TABLE nodes RENAME COLUMN firmware_version TO build_version;
ALTER TABLE nodes ADD COLUMN build_name TEXT NOT NULL DEFAULT '';
ALTER TABLE nodes ADD COLUMN build_target TEXT NOT NULL DEFAULT '';

-- Software nodes sent "<name> <version>"; the ESP32 firmware sent the version alone.
UPDATE nodes
SET build_name = substr(build_version, 1, instr(build_version, ' ') - 1),
    build_version = substr(build_version, instr(build_version, ' ') + 1)
WHERE instr(build_version, ' ') > 0;
UPDATE nodes SET build_name = 'firmware', build_target = 'esp32'
WHERE build_name = '' AND device_id LIKE 'esp32-%';
