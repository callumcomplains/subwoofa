//! The two 16-bit CRCs of the Dolby and DTS framings (most significant bit first).

const fn table(poly: u16) -> [u16; 256] {
    let mut t = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = (i as u16) << 8;
        let mut k = 0;
        while k < 8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ poly } else { c << 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static ANSI: [u16; 256] = table(0x8005);
static CCITT: [u16; 256] = table(0x1021);

fn run(t: &[u16; 256], init: u16, data: &[u8]) -> u16 {
    data.iter().fold(init, |c, b| (c << 8) ^ t[((c >> 8) as u8 ^ b) as usize])
}

/// AC-3 / E-AC-3: x^16 + x^15 + x^2 + 1, register starts at zero. A block followed by its own CRC leaves zero.
pub fn ac3(data: &[u8]) -> u16 {
    run(&ANSI, 0, data)
}

/// DTS headers: CCITT polynomial, register starts at 0xFFFF. A block followed by its own CRC leaves zero.
pub fn dts(data: &[u8]) -> u16 {
    run(&CCITT, 0xFFFF, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appending_the_crc_gives_zero() {
        let body = b"123456789";
        let c = run(&ANSI, 0, body);
        let mut with = body.to_vec();
        with.extend_from_slice(&c.to_be_bytes());
        assert_eq!(ac3(&with), 0);
        let c = run(&CCITT, 0xFFFF, body);
        assert_eq!(c, 0x29B1); // the CCITT-FALSE check value
        let mut with = body.to_vec();
        with.extend_from_slice(&c.to_be_bytes());
        assert_eq!(dts(&with), 0);
    }
}
