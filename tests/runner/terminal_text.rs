// ConPTY can insert CSI/OSC between text writes; this is not a screen emulator.
pub fn text(raw: &str) -> String {
    let mut output = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '\u{1b}' {
            output.push(character);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for character in chars.by_ref() {
                    if ('@'..='~').contains(&character) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(character) = chars.next() {
                    if character == '\u{7}' {
                        break;
                    }
                    if character == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    output
}

#[test]
fn conpty_controls_do_not_split_asserted_text() {
    let ci_output = "\u{1b}[?9001h\u{1b}[?1004h\u{1b}[?25l\u{1b}[2J\u{1b}[m\u{1b}[HTTY\u{1b}]0;C:\\hostedtoolcache\\windows\\Python\\3.14.7\\x64\\python.exe\u{7}\u{1b}[?25h True True\r\nREADY>";
    assert_eq!(text(ci_output), "TTY True True\r\nREADY>");
    assert_eq!(
        text("VALUE=中\u{1b}]0;title\u{1b}\\\u{1b}[32m文 test\u{1b}[0m\r\n"),
        "VALUE=中文 test\r\n"
    );
    assert_eq!(text("TTY False True"), "TTY False True");
    assert_eq!(text("\u{1b}]0;TTY True True\u{7}"), "");
    assert_eq!(text("READY>\u{1b}[?25"), "READY>");
    assert_eq!(text("READY>\u{1b}]0;unfinished"), "READY>");
}
