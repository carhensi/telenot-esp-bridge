/// FT1.2 frame checksum: the arithmetic sum of the user-data octets, modulo 256.
///
/// Verified against real telegrams captured from a live Telenot Complex 400H:
/// the intArm command user-data `73 01 05 02 00 05 31 02 62` sums to `0x15`.
pub fn checksum(user_data: &[u8]) -> u8 {
    let mut sum: u8 = 0;
    for &b in user_data {
        sum = sum.wrapping_add(b);
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::checksum;

    #[test]
    fn empty_is_zero() {
        assert_eq!(checksum(&[]), 0);
    }

    #[test]
    fn wraps_modulo_256() {
        assert_eq!(checksum(&[0xFF, 0x01]), 0x00);
        assert_eq!(checksum(&[0x80, 0x80, 0x01]), 0x01);
    }

    #[test]
    fn matches_real_intarm_userdata() {
        // 73 01 05 02 00 05 31 02 62  ->  0x15
        let user = [0x73, 0x01, 0x05, 0x02, 0x00, 0x05, 0x31, 0x02, 0x62];
        assert_eq!(checksum(&user), 0x15);
    }
}
