//! The serial console's command language.
//!
//! Commands act on the selected channel (`ch N`, default 0), so the same
//! commands work for a single pad and for a board wired to several HX711s.

/// A parsed console command.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    /// `help`: list commands.
    Help,
    /// `status`: firmware version, channels, calibration and tare state.
    Status,
    /// `ch N`: select the channel later commands act on.
    Channel(u8),
    /// `fs KG`: set the full-scale rating used for pass targets.
    FullScale(f32),
    /// `stream on|off`: start or stop CSV rows.
    Stream(bool),
    /// `tare`: zero at the current reading.
    Tare,
    /// `cal add KG`: record a calibration point with KG on the pad (once stable).
    CalAdd(f32),
    /// `cal undo`: drop the last point.
    CalUndo,
    /// `cal clear`: drop all points.
    CalClear,
    /// `cal fit`: fit the points, apply the result and print residuals.
    CalFit,
    /// `cal save`: write the points to flash.
    CalSave,
    /// `cal show`: list points and the active fit.
    CalShow,
    /// `test noise [N]`: standard deviation of N unloaded readings (default 100).
    TestNoise(u32),
    /// `test repeat [N] [KG]`: place and remove a weight N times (default 10),
    /// counting it as placed above KG (default 1).
    TestRepeat {
        /// Placements.
        placements: u32,
        /// Load threshold, kg.
        threshold_kg: f32,
    },
    /// `test offcenter [N] [KG]`: as `test repeat`, moving the weight between
    /// placements (default 5: center and each corner).
    TestOffCenter {
        /// Placements.
        placements: u32,
        /// Load threshold, kg.
        threshold_kg: f32,
    },
    /// `test creep [S]`: track drift under a held load for S seconds (default 600).
    TestCreep(u32),
    /// `test zero`: compare the next stable reading with zero.
    TestZero,
    /// `test stop`: abandon the running test.
    TestStop,
    /// `sim KG`: set the simulated load (simulator builds only).
    Sim(f32),
}

/// Why a line didn't parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Blank line.
    Empty,
    /// Unknown command word.
    Unknown,
    /// A required argument is missing; names it.
    Missing(&'static str),
    /// An argument isn't a valid number or option.
    BadArgument,
    /// More arguments than the command takes.
    TooManyArguments,
}

/// Help text for `help`, one command per line.
pub const HELP: &str = "\
status                   firmware, channels, calibration
ch N                     select channel N (default 0)
fs KG                    full-scale rating for pass targets (default 200)
stream on|off            CSV rows on/off
tare                     zero at the current reading
cal add KG               add a calibration point with KG on the pad
cal undo | clear         drop the last point | all points
cal fit                  fit and apply; prints residuals
cal save | show          write points to flash | list points and fit
test noise [N]           std dev of N unloaded readings (100)
test repeat [N] [KG]     place/remove a weight N times (10), loaded above KG (1)
test offcenter [N] [KG]  same, moving the weight each time (5)
test creep [S]           drift under a held load for S seconds (600)
test zero                next stable reading vs. zero
test stop                abandon the running test
sim KG                   simulated load (sim builds only)";

impl Command {
    /// Parses one line. Words are case-insensitive and separated by whitespace.
    pub fn parse(line: &str) -> Result<Self, ParseError> {
        let mut args = line.split_ascii_whitespace();
        let word = args.next().ok_or(ParseError::Empty)?;
        let cmd = match lower(word) {
            "help" | "?" => Command::Help,
            "status" => Command::Status,
            "ch" => Command::Channel(required(&mut args, "channel")?),
            "fs" => Command::FullScale(positive(required(&mut args, "kg")?)?),
            "stream" => match args.next().map(lower) {
                Some("on") => Command::Stream(true),
                Some("off") => Command::Stream(false),
                Some(_) => return Err(ParseError::BadArgument),
                None => return Err(ParseError::Missing("on|off")),
            },
            "tare" => Command::Tare,
            "sim" => Command::Sim(finite(required(&mut args, "kg")?)?),
            "cal" => match lower(args.next().ok_or(ParseError::Missing("subcommand"))?) {
                "add" => Command::CalAdd(finite(required(&mut args, "kg")?)?),
                "undo" => Command::CalUndo,
                "clear" => Command::CalClear,
                "fit" => Command::CalFit,
                "save" => Command::CalSave,
                "show" => Command::CalShow,
                _ => return Err(ParseError::Unknown),
            },
            "test" => match lower(args.next().ok_or(ParseError::Missing("test name"))?) {
                "noise" => Command::TestNoise(optional(&mut args, 100)?),
                "repeat" => Command::TestRepeat {
                    placements: optional(&mut args, 10)?,
                    threshold_kg: positive(optional(&mut args, 1.0)?)?,
                },
                "offcenter" => Command::TestOffCenter {
                    placements: optional(&mut args, 5)?,
                    threshold_kg: positive(optional(&mut args, 1.0)?)?,
                },
                "creep" => Command::TestCreep(optional(&mut args, 600)?),
                "zero" => Command::TestZero,
                "stop" => Command::TestStop,
                _ => return Err(ParseError::Unknown),
            },
            _ => return Err(ParseError::Unknown),
        };
        match args.next() {
            Some(_) => Err(ParseError::TooManyArguments),
            None => Ok(cmd),
        }
    }
}

/// Lowercases short ASCII words without allocating; longer words never match a command.
fn lower(word: &str) -> &'static str {
    const WORDS: &[&str] = &[
        "help",
        "?",
        "status",
        "ch",
        "fs",
        "stream",
        "on",
        "off",
        "tare",
        "cal",
        "add",
        "undo",
        "clear",
        "fit",
        "save",
        "show",
        "test",
        "noise",
        "repeat",
        "offcenter",
        "creep",
        "zero",
        "stop",
        "sim",
    ];
    WORDS.iter().find(|w| w.eq_ignore_ascii_case(word)).copied().unwrap_or("")
}

fn required<'a, T: core::str::FromStr>(
    args: &mut impl Iterator<Item = &'a str>,
    name: &'static str,
) -> Result<T, ParseError> {
    args.next().ok_or(ParseError::Missing(name))?.parse().map_err(|_| ParseError::BadArgument)
}

fn optional<'a, T: core::str::FromStr>(args: &mut impl Iterator<Item = &'a str>, default: T) -> Result<T, ParseError> {
    args.next().map_or(Ok(default), |a| a.parse().map_err(|_| ParseError::BadArgument))
}

fn finite(x: f32) -> Result<f32, ParseError> {
    if x.is_finite() { Ok(x) } else { Err(ParseError::BadArgument) }
}

fn positive(x: f32) -> Result<f32, ParseError> {
    if x.is_finite() && x > 0.0 { Ok(x) } else { Err(ParseError::BadArgument) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands() {
        let cases = [
            ("help", Command::Help),
            ("STATUS", Command::Status),
            ("ch 2", Command::Channel(2)),
            ("fs 2000", Command::FullScale(2000.0)),
            ("stream on", Command::Stream(true)),
            ("  stream   OFF ", Command::Stream(false)),
            ("tare", Command::Tare),
            ("cal add 20.5", Command::CalAdd(20.5)),
            ("cal add 0", Command::CalAdd(0.0)),
            ("Cal Fit", Command::CalFit),
            ("cal undo", Command::CalUndo),
            ("cal clear", Command::CalClear),
            ("cal save", Command::CalSave),
            ("cal show", Command::CalShow),
            ("test noise", Command::TestNoise(100)),
            ("test noise 250", Command::TestNoise(250)),
            ("test repeat", Command::TestRepeat { placements: 10, threshold_kg: 1.0 }),
            ("test repeat 6 2.5", Command::TestRepeat { placements: 6, threshold_kg: 2.5 }),
            ("test offcenter", Command::TestOffCenter { placements: 5, threshold_kg: 1.0 }),
            ("test creep", Command::TestCreep(600)),
            ("test creep 60", Command::TestCreep(60)),
            ("test zero", Command::TestZero),
            ("test stop", Command::TestStop),
            ("sim 20", Command::Sim(20.0)),
        ];
        for (line, want) in cases {
            assert_eq!(Command::parse(line), Ok(want), "{line:?}");
        }
    }

    #[test]
    fn reports_errors() {
        let cases = [
            ("", ParseError::Empty),
            ("   ", ParseError::Empty),
            ("frobnicate", ParseError::Unknown),
            ("cal", ParseError::Missing("subcommand")),
            ("cal add", ParseError::Missing("kg")),
            ("cal add heavy", ParseError::BadArgument),
            ("cal add NaN", ParseError::BadArgument),
            ("cal bake", ParseError::Unknown),
            ("ch 256", ParseError::BadArgument),
            ("ch -1", ParseError::BadArgument),
            ("fs 0", ParseError::BadArgument),
            ("stream maybe", ParseError::BadArgument),
            ("stream", ParseError::Missing("on|off")),
            ("test", ParseError::Missing("test name")),
            ("test noise -5", ParseError::BadArgument),
            ("test repeat 5 -1", ParseError::BadArgument),
            ("tare now", ParseError::TooManyArguments),
            ("sim", ParseError::Missing("kg")),
        ];
        for (line, want) in cases {
            assert_eq!(Command::parse(line), Err(want), "{line:?}");
        }
    }

    #[test]
    fn help_lists_every_command_word() {
        for word in ["status", "ch", "fs", "stream", "tare", "cal", "test", "sim"] {
            assert!(HELP.lines().any(|l| l.starts_with(word)), "{word}");
        }
    }
}
