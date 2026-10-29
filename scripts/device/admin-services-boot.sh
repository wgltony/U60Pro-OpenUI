#!/bin/sh
# OpenUI admin-services boot guard v1
# Boot policy only. Manual runtime starts explicitly bypass this init guard.
# No service, USB, network, authentication or key changes are made here.
case "$1" in adb|ssh) ;; *) exit 1 ;; esac
CONFIG=/etc/openui-admin-services.json
# Missing policy preserves the already-enabled native B31 services. Once a
# policy exists, unreadable/invalid/symlink/nonregular state must fail closed.
[ ! -L "$CONFIG" ] || exit 1
if [ ! -e "$CONFIG" ]; then exit 0; fi
[ -f "$CONFIG" ] && [ -r "$CONFIG" ] || exit 1
metadata=$(/bin/stat -c '%u:%a' "$CONFIG" 2>/dev/null) || exit 1
case "$metadata" in 0:*) mode=${metadata#0:} ;; *) exit 1 ;; esac
case "$mode" in ''|*[!0-7]*) exit 1 ;; esac
[ $((0$mode & 077)) -eq 0 ] || exit 1
exec /usr/bin/lua - "$1" <<'LUA'
local path = '/etc/openui-admin-services.json'
local f = io.open(path, 'rb')
if not f then os.exit(1) end
local text = f:read(4097)
f:close()
if not text or #text > 4096 then os.exit(1) end
for i = 1, #text do
  local c = text:byte(i)
  if c >= 127 or (c < 32 and c ~= 9 and c ~= 10 and c ~= 13) then os.exit(1) end
end
-- Tiny strict schema parser: exactly three plain keys, JS-safe u64 revision,
-- and two booleans. No duplicate keys, escapes, extra fields or trailing data.
local pos, values, count = 1, {}, 0
local function fail() os.exit(1) end
local function token(pattern)
  local a, b, value = text:find(pattern, pos)
  if a ~= pos then fail() end
  pos = b + 1
  return value
end
token('%s*{%s*')
while true do
  local key = token('"([a-z_]+)"%s*:%s*')
  if values[key] ~= nil then fail() end
  if key == 'revision' then
    local number = token('(%d+)')
    if (#number > 1 and number:sub(1, 1) == '0') or #number > 16 or
       (#number == 16 and number > '9007199254740991') then fail() end
    values[key] = number
  elseif key == 'adb_boot' or key == 'ssh_boot' then
    local value = token('([a-z]+)')
    if value ~= 'true' and value ~= 'false' then fail() end
    values[key] = value == 'true'
  else fail() end
  count = count + 1
  token('%s*')
  local delimiter = text:sub(pos, pos)
  pos = pos + 1
  if delimiter == '}' then break end
  if delimiter ~= ',' then fail() end
  token('%s*')
end
if count ~= 3 or values.revision == nil or values.adb_boot == nil or
   values.ssh_boot == nil or not text:sub(pos):match('^%s*$') then fail() end
os.exit(values[arg[1] .. '_boot'] == true and 0 or 1)
LUA
