//! Bit reader over a byte slice (most significant bit first). Reading past the end yields zeros and sets `over`.

pub struct Bits<'a> {
    d: &'a [u8],
    pub pos: usize,
    pub over: bool,
}

impl<'a> Bits<'a> {
    pub fn new(d: &'a [u8]) -> Self {
        Bits { d, pos: 0, over: false }
    }
    pub fn get(&mut self, n: usize) -> u32 {
        let mut v = 0u64;
        for _ in 0..n {
            let byte = match self.d.get(self.pos >> 3) {
                Some(b) => *b,
                None => {
                    self.over = true;
                    0
                }
            };
            v = (v << 1) | ((byte >> (7 - (self.pos & 7))) & 1) as u64;
            self.pos += 1;
        }
        v as u32
    }
    pub fn flag(&mut self) -> bool {
        self.get(1) == 1
    }
    pub fn skip(&mut self, n: usize) {
        self.pos += n;
        if self.pos > self.d.len() * 8 {
            self.over = true;
        }
    }
    pub fn left(&self) -> usize {
        (self.d.len() * 8).saturating_sub(self.pos)
    }
}
