pub fn pcm16_bytes(samples: &[i16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_pcm16_as_little_endian() {
        assert_eq!(pcm16_bytes(&[0x1234, -2]), [0x34, 0x12, 0xfe, 0xff]);
    }
}
