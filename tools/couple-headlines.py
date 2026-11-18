"""Headlines for the two of us: anniversaries, Valentine's Day and one line
a week. Read by generate-headlines.py; a key here replaces the same key in
special-headlines.py (Valentine's Day uses only these lines).

{years} is the time since the day itself ("1 year", "3 years").
An empty subtitle shows the title alone.
"""

COUPLE = {
    # October 16, 2023.
    "first_kiss": [
        ("First kiss: {years} ago.", "October 16, 2023. Best commit you ever made."),
        ("Kiss.exe, version 1.0.", "Released October 16, 2023. Still the best build."),
        ("Uptime since the first kiss: {years}.", "Zero regrets logged."),
        ("Happy first-kiss day.", "Go find the Wife and recreate it."),
        ("Day one of the best feature.", "{years} ago today. Kiss the Ms."),
        ("Anniversary of a first kiss.", "{years} since you and wifey started this repo."),
    ],
    # March 27, 2024.
    "dating": [
        ("Relationship uptime: {years}.", "Dating since March 27, 2024. Happy anniversary."),
        ("Happy dating anniversary.", "{years} since it all started with wifey."),
        ("Pairing anniversary.", "Connected to wifey since March 27, 2024."),
        ("Best merge in the history of the repo.", "{years} together today."),
        ("Anniversary patch applied.", "Plan something nice for the Ms."),
        ("Still the best connection on the network.", "{years} and counting."),
    ],
    # December 31, 2024.
    "proposal": [
        ("Proposal anniversary.", "December 31, 2024: she said yes."),
        ("She said yes, {years} ago.", "New Year's Eve will never be just New Year's Eve."),
        ("Best year-end commit ever.", "{years} since you proposed to Airbear."),
        ("Request: marry me. Response: 200 OK.", "Happy proposal anniversary."),
        ("Countdown to midnight, and to forever.",
         "{years} since the proposal. Kiss the Wife at 00:00."),
        ("Ring.exe shipped on New Year's Eve.", "{years} ago tonight. Still the best release."),
    ],
    "valentine": [
        ("Love.exe: running for wifey.", "Happy Valentine's Day."),
        ("Compatibility check: Ariana, 100%.", "Happy Valentine's Day."),
        ("Valentine's Day, married edition.", "Cuddles scheduled. Snuggles pending."),
        ("Heart rate: elevated.", "Happy Valentine's Day to the Ms."),
        ("<3 merged to main, permanently.", "Happy Valentine's Day, sweetie."),
        ("Pairing successful.", "Valentine's Day. Bluetooth wishes it was this connected."),
        ("Movie night unlocked.", "Titanic or The Notebook? Valentine's rules: wifey picks."),
    ],
    # One hour a week, at a different time each week.
    "us": [
        ("Incoming ping from Cali.", "\"Why can't you just live in California?\""),
        ("Packet received: I miss you.", "Long-distance latency, short-distance love."),
        ("Rite Aid: deprecated.", "\"Rite Aid is getting really old.\""),
        ("Just a Rite Aid girl, living in a Nissan world.", ""),
        ("Bed size: too big.", "\"My bed feels too big.\" Not anymore."),
        ("Scared mode: off.", "\"I'm scared.\" Reply sent: I'm right here."),
        ("Signal lost: why aren't you here?", "Patched. Distance was just a bug."),
        ("Missing-you counter: reset.", "No more \"I miss you\" across state lines."),
        ("Cuddle protocol initiated.", "Snuggles have priority over all other processes."),
        ("Snuggle buffer: full.", "Couch, blanket, wifey. In that order."),
        ("Trail mode: enabled.", "Pick a hike. Bring wifey and the snacks."),
        ("Nature: please render.", "Hike with sweetie. Phones on airplane mode."),
        ("Wanderlust.exe running.", "Cozumel or Honduras next? Ask wifey."),
        ("Flashback: Cozumel.", "Clear water, warm sun, best travel buddy."),
        ("Throwback: Honduras.", "Adventure logged with wifey."),
        ("Oregon coast loading.", "Oceanside sunsets with Airbear."),
        ("Route saved: Medford, OR.", "Good memories, cached forever."),
        ("Waco, TX: on the map.", "Every place counts when sweetie's there."),
        ("Houston, we have a cuddle.", "Houston, TX, wifey edition."),
        ("Sacramento: in the travel log.", "Every mile was worth it for wifey."),
        ("Travel plans pending.", "Next destination: wherever the Wife wants."),
        ("Wifey.exe: favorite app.", "Always running, never closing."),
        ("Priority one: wifey.", "Everything else can wait."),
        ("Stars Hollow mode.", "Gilmore Girls marathon. Coffee for two."),
        ("Coffee level: Lorelai.", "Gilmore Girls night with the Ms."),
        ("I'm the king of the world.", "Titanic night? Wifey picks the blanket."),
        ("Hibernation cancelled.", "Passengers night with wifey."),
        ("If you're a bird, I'm a bird.", "The Notebook night. Tissues on standby."),
        ("Hey baby, movie night?", "Gilmore Girls, Titanic or The Notebook?"),
    ],
}
