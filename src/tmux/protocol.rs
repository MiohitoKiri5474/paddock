//! Parsing tmux control-mode output (`tmux -C`), independent of any process.

/// One thing the control client tells us.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// Raw bytes a pane's program wrote.
    Output { pane: String, data: Vec<u8> },
    /// The reply to one command this client sent, in the order sent.
    Reply { ok: bool, lines: Vec<Vec<u8>> },
    /// The control client ended (detached, or its session was destroyed).
    Exit,
    /// Any other notification (`%layout-change`, `%window-renamed`, ...).
    Notification(String),
}

/// Decode `%output` escaping: bytes below 32 and `\` arrive as `\ooo` octal.
pub fn unescape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'\\' && i + 4 <= data.len() {
            let d = &data[i + 1..i + 4];
            if d.iter().all(|b| (b'0'..=b'7').contains(b)) {
                out.push((d[0] - b'0') * 64 + (d[1] - b'0') * 8 + (d[2] - b'0'));
                i += 4;
                continue;
            }
        }
        out.push(data[i]);
        i += 1;
    }
    out
}

/// Turns control-mode lines into [`Message`]s. Lines inside a
/// `%begin`/`%end` block are collected into one reply.
#[derive(Default)]
pub struct Parser {
    /// Inside a block: whether it answers our own command, and its lines so far.
    block: Option<(bool, Vec<Vec<u8>>)>,
}

impl Parser {
    /// Feed one line (without its trailing newline).
    pub fn line(&mut self, line: &[u8]) -> Option<Message> {
        if let Some((mine, lines)) = self.block.as_mut() {
            let end = line.starts_with(b"%end ");
            if end || line.starts_with(b"%error ") {
                let (mine, lines) = (*mine, std::mem::take(lines));
                self.block = None;
                return mine.then_some(Message::Reply { ok: end, lines });
            }
            lines.push(line.to_vec());
            return None;
        }
        if let Some(rest) = line.strip_prefix(b"%output ") {
            let split = rest.iter().position(|&b| b == b' ').unwrap_or(rest.len());
            let pane = String::from_utf8_lossy(&rest[..split]).into_owned();
            let data = unescape(rest.get(split + 1..).unwrap_or_default());
            return Some(Message::Output { pane, data });
        }
        if line.starts_with(b"%begin ") {
            // `%begin <time> <number> <flags>`: flags 1 means this client sent it.
            // The block tmux prints for the attach itself has flags 0.
            self.block = Some((line.ends_with(b" 1"), Vec::new()));
            return None;
        }
        if line.starts_with(b"%exit") {
            return Some(Message::Exit);
        }
        Some(Message::Notification(
            String::from_utf8_lossy(line).into_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescape_decodes_octal_and_keeps_utf8() {
        assert_eq!(unescape(br"a\033[1mb\015\012"), b"a\x1b[1mb\r\n");
        assert_eq!(unescape(br"\134"), b"\\");
        assert_eq!(unescape("漢✳".as_bytes()), "漢✳".as_bytes());
        assert_eq!(unescape(br"\03"), br"\03");
        assert_eq!(unescape(br"\9xx"), br"\9xx");
    }

    #[test]
    fn output_lines_carry_pane_and_bytes() {
        let mut p = Parser::default();
        assert_eq!(
            p.line(br"%output %3 hi\015\012"),
            Some(Message::Output {
                pane: "%3".into(),
                data: b"hi\r\n".to_vec()
            })
        );
        assert_eq!(
            p.line(b"%output %3"),
            Some(Message::Output {
                pane: "%3".into(),
                data: vec![]
            })
        );
    }

    #[test]
    fn own_blocks_become_replies_and_foreign_blocks_are_dropped() {
        let mut p = Parser::default();
        assert_eq!(p.line(b"%begin 1 10 0"), None);
        assert_eq!(p.line(b"%end 1 10 0"), None);
        assert_eq!(p.line(b"%begin 1 11 1"), None);
        assert_eq!(p.line(b"line one"), None);
        assert_eq!(p.line(b"%output %1 not output inside a block"), None);
        assert_eq!(
            p.line(b"%end 1 11 1"),
            Some(Message::Reply {
                ok: true,
                lines: vec![
                    b"line one".to_vec(),
                    b"%output %1 not output inside a block".to_vec()
                ]
            })
        );
        assert_eq!(p.line(b"%begin 1 12 1"), None);
        assert_eq!(
            p.line(b"%error 1 12 1"),
            Some(Message::Reply {
                ok: false,
                lines: vec![]
            })
        );
    }

    #[test]
    fn exit_and_notifications() {
        let mut p = Parser::default();
        assert_eq!(p.line(b"%exit"), Some(Message::Exit));
        assert_eq!(p.line(b"%exit server exited"), Some(Message::Exit));
        assert_eq!(
            p.line(b"%window-renamed @1 zsh"),
            Some(Message::Notification("%window-renamed @1 zsh".into()))
        );
    }
}
