#!/usr/bin/env python3
"""Writes src/weather-headlines.tsv: the Tech weather headlines Luma picks from.

Every hand-written line in src/weather-phrases.md is kept. On top of them,
each weather gets generated headlines: a tech noun with a state that fits the
weather ("Router: water-cooled.") over a plain line about the weather with a
piece of advice that suits it ("Steady rain. Good day for a long build.").
The seed is fixed, so the file only changes when this script does.

    python3 tools/generate-headlines.py
"""
import random
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "src" / "weather-phrases.md"
OUT = ROOT / "src" / "weather-headlines.tsv"
PER_WEATHER = 920  # headlines per weather, hand-written ones included

NOUNS = """Router Firewall Kernel Cache Buffer Server Compiler Debugger Driver
Firmware Modem Wi-Fi Bluetooth GPU CPU SSD RAM Backup Codec Shader Bitrate
Bandwidth Ping Uptime Terminal Patch Hotfix Download Upload Stream Battery
Thermostat Database Cursor Keyboard Remote Playlist Inbox Calendar Antenna
Satellite Subwoofer Projector Webcam Motherboard Heatsink Scheduler Sensor
Watchdog Daemon Pixel Sync Login Clipboard Algorithm""".split()
NOUNS += ["Power supply", "Cooling fan", "Frame rate", "Render queue", "Cron job",
          "Log file", "Swap file", "Status page", "Build server", "Smart plug",
          "Home hub", "Screen saver", "Media server", "Weather API", "Sky renderer"]

STATES = {
    "clear_nice": ["fully optimized", "running at peak", "zero errors", "basking in the sun",
                   "green across the board", "in HDR mode", "at max brightness",
                   "no clouds detected", "compiled cleanly", "solar powered",
                   "all systems go", "100% uptime", "sunlight enabled", "patched and happy",
                   "low latency, high spirits"],
    "chilly": ["running cool", "at idle temps", "in passive cooling", "pleasantly chilled",
               "fan speed low", "in a cool-down cycle", "in hoodie mode", "underclocked",
               "brisk", "at optimal temps", "in power-save mode", "running crisp",
               "cooled to spec", "thermal headroom: plenty"],
    "cold": ["cold booting", "thermal paste solid", "frosty", "warming up slowly",
             "in low-temp mode", "cache gone cold", "lagging in the cold", "needs a jacket",
             "at 2% warmth", "shivering", "stuck in cold start", "hands too cold to type",
             "needs a hot drink", "fans not required"],
    "frost": ["frozen", "below zero", "iced over", "in cryo mode", "fully frozen",
              "frost detected", "stuck in ice", "liquid nitrogen ready", "kernel-level frost",
              "overclocking weather", "frozen solid", "ice-cold", "in cold storage",
              "not responding (frozen)"],
    "hot": ["overheating", "thermal throttling", "at 95°C", "melting", "fans at 100%",
            "running hot", "overclocked by the sun", "needs liquid cooling",
            "past thermal limits", "reflow temperature", "in meltdown mode",
            "sweating bits", "throttled to survive", "hot enough to fry an egg"],
    "cloudy": ["in the cloud", "low contrast", "in grey mode", "sun not found", "dimmed",
               "overcast", "syncing clouds", "at 20% brightness", "in default grey theme",
               "waiting for the sun", "fully clouded", "sun.dll missing", "cloud storage full",
               "rendering in greyscale"],
    "rain": ["water-cooled", "leaking", "in drip mode", "soaked", "at risk of liquid damage",
             "rain detected", "needs waterproofing", "in puddle mode", "liquid cooling online",
             "downloading rain", "dropping packets", "showering", "running wet",
             "splash-proof mode"],
    "thunder": ["power surge", "voltage spikes", "static discharge", "UPS on standby",
                "struck by lightning", "surge protected", "brownout risk",
                "rebooting in the storm", "lightning-fast", "grounded, hopefully",
                "storm mode", "thunder detected", "unplug recommended", "high voltage"],
    "snow": ["snowed in", "white noise", "particle effects on", "frozen frame",
             "pixel-perfect snow", "buffering snowflakes", "covered in static",
             "snow shader on", "in winter mode", "all white", "drifting",
             "flaking", "snowflake overflow", "cold and fluffy"],
    "wind": ["max airflow", "in turbo fan mode", "gusting", "blown away",
             "high bandwidth breeze", "airflow excessive", "in a wind tunnel test",
             "cooling overload", "spinning up", "losing packets to the wind",
             "draft mode", "fans outdoors", "breezy", "wind speed: high"],
    "fog": ["low resolution", "blurred", "draw distance 2 m", "bitrate starved",
            "in fog mode", "depth of field maxed", "visibility 240p", "lost in the haze",
            "out of focus", "rendering slowly", "in soft focus", "misted over",
            "hazy", "packet loss: visual"],
    "night": ["in sleep mode", "dark mode on", "on night shift", "dimmed",
              "in low-power mode", "running nightly builds", "dreaming in code",
              "on standby", "idle", "in quiet hours", "after hours", "powering down",
              "screen time: over", "on the night shift"],
}

WEATHER_LINES = {
    "clear_nice": ["Clear skies.", "Sunny and pleasant.", "Blue sky all day.", "Nice out.",
                   "Not a cloud around.", "Bright and mild.", "Perfect weather."],
    "chilly": ["It's chilly.", "Cool out there.", "A little brisk.", "Crisp air outside.",
               "Jacket weather.", "Cool and clear-ish."],
    "cold": ["It's cold.", "Cold out there.", "Near freezing.", "Coat weather.",
             "Properly cold today.", "Cold and still."],
    "frost": ["Below freezing.", "Frost outside.", "Sub-zero out there.", "Icy out.",
              "Frozen solid outside.", "Below zero."],
    "hot": ["It's hot.", "Scorching out there.", "A real heat wave.", "Hot outside.",
            "The sun is not holding back.", "Hot and bright."],
    "cloudy": ["Grey skies.", "Overcast.", "Clouds all day.", "Grey and dull.",
               "No sun in sight.", "Cloudy out."],
    "rain": ["It's raining.", "Rain all day.", "Wet out there.", "Showers outside.",
             "Steady rain.", "Raining on and off."],
    "thunder": ["Thunderstorms.", "Storms overhead.", "Lightning nearby.",
                "Thunder outside.", "Stormy out there.", "A storm is passing through."],
    "snow": ["It's snowing.", "Snow everywhere.", "Fresh snow outside.", "Snowing steadily.",
             "Flurries outside.", "A snowy one."],
    "wind": ["It's windy.", "Strong winds.", "Gusty out there.", "Blustery today.",
             "Windy outside.", "The wind is up."],
    "fog": ["Foggy.", "Thick fog outside.", "Misty out there.", "Low visibility.",
            "Fog everywhere.", "Hazy and grey."],
    "night": ["It's late.", "Late night.", "The night is quiet.", "Past bedtime.",
              "Dark outside.", "Night mode, outside too."],
}

INDOOR = ["Good day for a long build.", "Queue up a movie.", "Stay in and stream something.",
          "Blanket, couch, playlist.", "The couch has root access.", "Perfect for a series.",
          "Hot drink recommended.", "Stay on the wired connection.", "Save your work.",
          "Keep the electronics indoors.", "Time for one more episode.",
          "Ideal for catching up on a backlog.", "Indoors is the best server room.",
          "A good excuse to stay in.", "Pick a film, any film.", "Commit and stay in.",
          "Lights low, volume up.", "The remote is your best friend today."]
OUTDOOR = ["Go outside. It's a feature, not a bug.", "Touch some grass.",
           "Take the laptop to the porch.", "Screen time can wait an hour.",
           "Ship it, then go outside.", "Rare build. Enjoy it.", "Peak outdoor uptime.",
           "A walk between builds sounds right.", "Leave the router to it.",
           "Sunglasses recommended.", "Even the router looks happy.",
           "Log off for a bit."]

TAILS = {
    "clear_nice": OUTDOOR,
    "chilly": OUTDOOR[:6] + ["Grab a hoodie.", "Long sleeves required.", "Layer up.",
                             "Good temps for a long build."],
    "cold": INDOOR + ["Scarf, gloves, the full stack.", "Coat required.",
                      "Let your fingers warm up first."],
    "frost": INDOOR + ["Scrape the windshield first.", "Watch for ice.",
                       "Everything is slow to start."],
    "hot": ["Find the AC.", "Hydrate like your power supply depends on it.",
            "Stay inside.", "Sunscreen is the firewall.", "Ice water and a cool screen.",
            "Keep the fans spinning.", "Shade is a feature.", "Stay near the AC.",
            "Hot as a gaming laptop on your lap.", "Cold drinks recommended."]
           + INDOOR[1:8],
    "cloudy": INDOOR + ["Try again later.", "The TV is the brightest thing around.",
                        "The sun is on a coffee break."],
    "rain": INDOOR + ["Umbrella recommended.", "Shoes off, screen on.",
                      "Rain on the window, popcorn on the couch."],
    "thunder": ["Unplug the expensive stuff.", "Save your work.",
                "Keep the router on a surge protector.", "Turn the volume up.",
                "Stay indoors.", "Check your UPS.", "Back up, just in case."] + INDOOR[1:10],
    "snow": INDOOR + ["Enjoy the render.", "Roads in safe mode.", "Let it finish buffering."],
    "wind": INDOOR + ["Hold on to your headphones.", "Secure the satellite dish.",
                      "Hats are a risk.", "Tie down anything light."],
    "fog": INDOOR + ["Headlights on, expectations low.", "Drive slow.",
                     "Lower your expectations."],
    "night": ["One more episode?", "Even servers need maintenance windows.",
              "The backups can run without you.", "Keep the volume down.",
              "The world is idle. You could be too.", "The couch has root access.",
              "Go to bed. The logs will keep.", "Lights low, story high.",
              "Sleep is a feature.", "Dim the screen, rest the eyes."],
}


def handwritten():
    """Every headline in weather-phrases.md: each fixed pair, and every
    mixable title with every mixable subtitle of its section."""
    current, kind = None, ""
    titles, subtitles, out = {}, {}, []
    for line in SOURCE.read_text(encoding="utf-8").splitlines():
        if line.startswith("### "):
            kind = line[4:].strip()
        elif line.startswith("## "):
            current, kind = line[3:].split()[0], ""
        elif current and line.startswith("- "):
            item = line[2:].strip()
            if kind == "Titles":
                titles.setdefault(current, []).append(item)
            elif kind == "Subtitles":
                subtitles.setdefault(current, []).append(item)
            elif kind == "Pairs" and " — " in item:
                title, subtitle = item.split(" — ", 1)
                out.append((current, title, subtitle))
    for weather, ts in titles.items():
        for t in ts:
            for s in subtitles.get(weather, []):
                out.append((weather, t, s))
    return out


def main():
    rng = random.Random(20261118)
    rows = handwritten()
    seen = {(w, t, s) for w, t, s in rows}
    for weather, states in STATES.items():
        titles = [f"{noun}: {state}." for noun in NOUNS for state in states]
        subs = [f"{line} {tail}" for line in WEATHER_LINES[weather] for tail in TAILS[weather]]
        have = sum(1 for w, _, _ in rows if w == weather)
        rng.shuffle(titles)
        used_titles = set()
        tries = 0
        while have < PER_WEATHER and tries < 200000:
            tries += 1
            title = titles[tries % len(titles)]
            # Each generated title once, so the noun/state mix stays varied.
            if title in used_titles:
                continue
            row = (weather, title, rng.choice(subs))
            if row in seen:
                continue
            used_titles.add(title)
            seen.add(row)
            rows.append(row)
            have += 1
    for _, t, s in rows:
        assert "\t" not in t + s and "—" not in t + s, (t, s)
    order = list(STATES) + ["still", "change"]
    rows.sort(key=lambda r: order.index(r[0]))
    with OUT.open("w", encoding="utf-8") as f:
        f.write("# Generated by tools/generate-headlines.py; edit that or weather-phrases.md.\n")
        for weather, title, subtitle in rows:
            f.write(f"{weather}\t{title}\t{subtitle}\n")
    print(f"{len(rows)} headlines -> {OUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
