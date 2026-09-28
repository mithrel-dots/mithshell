use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use async_channel::Sender;
use log::warn;

pub const VISUALIZER_BARS: usize = 7;
pub type VisualizerLevels = [u8; VISUALIZER_BARS];

const CAVA_CONFIG: &str = r#"
[general]
framerate = 30
bars = 7
autosens = 1
sleep_timer = 1

[input]
method = pipewire
source = auto

[output]
method = raw
raw_target = /dev/stdout
data_format = ascii
ascii_max_range = 100
bar_delimiter = 59
frame_delimiter = 10
channels = mono

[smoothing]
noise_reduction = 80
"#;

pub fn start_visualizer(sender: Sender<VisualizerLevels>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        loop {
            let mut child = match Command::new("cava")
                .args(["-p", "/dev/stdin"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(child) => child,
                Err(error) => {
                    warn!("real media visualization is unavailable: failed to run cava: {error}");
                    thread::sleep(Duration::from_secs(10));
                    continue;
                }
            };

            let configured = child
                .stdin
                .take()
                .is_some_and(|mut input| input.write_all(CAVA_CONFIG.as_bytes()).is_ok());
            let Some(stdout) = child.stdout.take().filter(|_| configured) else {
                let _ = child.kill();
                thread::sleep(Duration::from_secs(5));
                continue;
            };
            for line in BufReader::new(stdout).lines().map_while(|line| line.ok()) {
                let Some(levels) = parse_cava_line(&line) else {
                    continue;
                };
                if sender.send_blocking(levels).is_err() {
                    let _ = child.kill();
                    return;
                }
            }
            let _ = child.wait();
            if sender.is_closed() {
                return;
            }
            warn!("cava media visualizer stopped; reconnecting");
            thread::sleep(Duration::from_secs(2));
        }
    })
}

fn parse_cava_line(line: &str) -> Option<VisualizerLevels> {
    let values: Vec<_> = line
        .split(';')
        .filter(|value| !value.is_empty())
        .map(|value| value.parse::<u8>().ok().map(|value| value.min(100)))
        .collect::<Option<_>>()?;
    values.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cava_ascii_frames() {
        assert_eq!(
            parse_cava_line("0;12;45;100;82;9;3;"),
            Some([0, 12, 45, 100, 82, 9, 3])
        );
        assert_eq!(parse_cava_line("0;1;2;"), None);
    }
}
