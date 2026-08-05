-- Rig control for RSSSTV.
--
-- This is the script the application runs when no rigcontrol.lua has been
-- written beside config.toml. Write one out from the Rig Control menu to take
-- it over; from then on your copy is what runs.
--
-- Every function here is optional. A module without transmit keys nothing,
-- which is what a station running VOX wants.
--
-- Each is called with a context table:
--
--   ctx.ports      the transports from [rig.ports], by name
--   ctx.band       the band the rig is on, or nil between bands
--   ctx.frequency  the last frequency read, in hertz, or nil
--   ctx.log(text)  writes to the application log
--
-- A rigctld port takes one command per call. Two commands on one line may be
-- answered twice, and the answer left over would be read by the next command.

local function transmit(ctx)
  ctx.ports.rig:send("T 1")
end

local function receive(ctx)
  ctx.ports.rig:send("T 0")
end

local function poll_frequency(ctx)
  return ctx.ports.rig:frequency()
end

local function set_frequency(ctx, hz)
  ctx.ports.rig:send(("F %d"):format(hz))
end

-- The band is the whole entry from bands.toml, so anything you add there
-- arrives here beside these. Hyphens become underscores on the way.
local function change_band(ctx, band)
  if band.target then
    set_frequency(ctx, band.target)
  end
  if band.receive_mode then
    ctx.ports.rig:send(("M %s %d"):format(band.receive_mode, band.bandwidth or 0))
  end
end

return {
  transmit = transmit,
  receive = receive,
  poll_frequency = poll_frequency,
  set_frequency = set_frequency,
  change_band = change_band,
}
