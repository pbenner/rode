//! DEC character-set state; stored with the screen and saved cursor attributes.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Set {
    #[default]
    Ascii,
    Uk,
    Graphics,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Charsets {
    sets: [Set; 2],
    active: usize,
}

impl Charsets {
    pub(crate) fn designate(&mut self, slot: usize, byte: u8) -> bool {
        let set = match byte {
            b'B' => Set::Ascii,
            b'A' => Set::Uk,
            b'0' => Set::Graphics,
            _ => return false,
        };
        self.sets[slot] = set;
        true
    }

    pub(crate) fn select(&mut self, slot: usize) {
        self.active = slot;
    }

    pub(crate) fn translate(self, c: char) -> char {
        match self.sets[self.active] {
            Set::Ascii => c,
            Set::Uk if c == '#' => '£',
            Set::Uk => c,
            Set::Graphics => match c {
                '_' => ' ',
                '`' => '◆',
                'a' => '▒',
                'b' => '␉',
                'c' => '␌',
                'd' => '␍',
                'e' => '␊',
                'f' => '°',
                'g' => '±',
                'h' => '␤',
                'i' => '␋',
                'j' => '┘',
                'k' => '┐',
                'l' => '┌',
                'm' => '└',
                'n' => '┼',
                'o' => '⎺',
                'p' => '⎻',
                'q' => '─',
                'r' => '⎼',
                's' => '⎽',
                't' => '├',
                'u' => '┤',
                'v' => '┴',
                'w' => '┬',
                'x' => '│',
                'y' => '≤',
                'z' => '≥',
                '{' => 'π',
                '|' => '≠',
                '}' => '£',
                '~' => '·',
                _ => c,
            },
        }
    }

    pub(crate) fn write_reset(contents: &mut Vec<u8>) {
        contents.extend_from_slice(b"\x1b(B\x1b)B\x0f");
    }

    pub(crate) fn write_restore(self, contents: &mut Vec<u8>) {
        for (slot, set) in self.sets.iter().enumerate() {
            if *set != Set::Ascii {
                contents.extend_from_slice(&[
                    0x1b,
                    if slot == 0 { b'(' } else { b')' },
                    if *set == Set::Uk { b'A' } else { b'0' },
                ]);
            }
        }
        if self.active == 1 {
            contents.push(0x0e);
        }
    }
}
