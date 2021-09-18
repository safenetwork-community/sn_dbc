use crate::{Error, Result};
use rug::ops::DivRounding;
use rug::ops::Pow;
use rug::Integer;
use rug::Rational;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::convert::TryFrom;
use std::fmt;
use std::hash::{Hash, Hasher};

// represents the exponent in 10^-10, 10^0, 10^3, etc.  -127..127.
pub type PowerOfTen = i8;

// defines size of unsigned counter for an Amount.
pub type AmountCounter = u32;

/// Represents a numeric amount as a count of a power of 10 unit.
/// eg:  1530 could be represented as any of:
///     Amount{count: 153, unit: 1}
///     Amount{count: 1530, unit: 0}
///     Amount{count: 15300, unit: -1}
///     Amount{count: 153000, unit: -2}
///     Amount{count: 1530000, unit: -3}
///     Amount{count: 15300000, unit: -4}
///     Amount{count: 153000000, unit: -5}
///
/// The maximum value of count is 1 billion.  This number is chosen because
/// using USD or Euro as examples, we observe that most real world transactions
/// can be performed for under 1 billion without need to change the unit.
///
/// The general idea is that we can start out with DBC denominations
/// at 10^0, and conduct transactions worth up to 1 billion tokens.
///
/// If one needs to spend more than one 1 billion, then one must
/// use a higher unit, 10^1 at minimum.
///
/// Likewise, if/when the currency gains in real world value, a smaller
/// amount of tokens can buy more.  As such, it can then make sense to
/// specify amounts with a smaller (negative exponent) unit.  This can happen
/// organically, as with the USD dollar where people used to buy eggs with
/// a nickel or dime, but now buy them with dollars.  The market has moved
/// to a higher unit simply because that requires the least number of coins/tokens.
///
/// For a deflationary currency gaining in value such as our DBC's are expected to be,
/// the market movement should normally be towards smaller units rather than larger.
///
/// Any two Amount can be added and subtracted only if they can both be represented
/// in the same unit, without the operation under or overflowing counter.
///
/// In other words, it does not make sense to try and add eg:
///     Amount{count: 15, unit: 0}     (aka 15)  and
///     Amount{count: 1, unit: -20}    (aka 0.00000000000000000001)
///
/// If we normalize both of these to unit: -20, then we have:
///     Amount{count: 1500000000000000000000, unit: -20}     (aka 15)  and
///     Amount{count: 1, unit: -20}    (aka 0.00000000000000000001)
///
/// However 1500000000000000000000 overflows our counter, which only
/// allows values up to 1 billion.  Hence these two amounts are incompatible.
///
/// Since the amounts cannot even add or subtract if they are not close
/// enough together, the Mint will not be able to sum inputs or outputs
/// that are too far apart and will issue an error.
///
/// This prevents users/wallets from generating huge amounts
/// of change with very unlike denominations, eg by subtracting 1 10^-30 from 1 10^3
/// This is a problem when using eg u128 to represent Amounts.  In the worst case with
/// u128 approx 40 outputs can be created when subtracting 1u128 from u128::MAX.
///
/// By comparison, using this Amount scheme with 1 billion max count, the max
/// change outputs is 9.
///
/// Unfortunately when using random number generators for quicktest test cases,
/// the common case becomes near the worst case.  Also, large numbers of inputs and
/// outputs create load on our system, in particular for signing and verifying.
/// Thus we are incentivized to keep the number of change coins as low as we
/// reasonably can.
///
/// In effect, this Amount scheme makes it hard for users to generate essentially
/// worthless dust amounts by accident. It is possible to do if one really tries
/// by reissuing in ever smaller amounts, but wallet software should generally
/// be trying NOT to do that.  And if a user does manage it, then s/he will have
/// difficulty using them in transactions with other people.  Fortunately the
/// reverse process can be used to bring them up into a "normal" range again.
///
#[derive(Clone, Debug, Copy, Default, Serialize, Deserialize)]
pub struct Amount {
    pub count: AmountCounter,
    pub unit: PowerOfTen,
}

/// A NormalizedAmount is just like an Amount except that count is a Big Integer.
/// So sum or difference of any two Amounts sharing the same unit can be represented
/// with a NormalizedAmount.
///
/// For now, have this only for internal use/ops.
#[derive(Debug)]
struct NormalizedAmount {
    count: Integer,
    unit: PowerOfTen,
}

impl Amount {
    pub fn new(count: AmountCounter, unit: PowerOfTen) -> Self {
        // We constrain count to ::counter_max().  If you want to use a bigger value,
        // you must change the unit.
        debug_assert!(count <= Self::counter_max());

        Self { count, unit }
    }

    pub fn counter_max() -> AmountCounter {
        // A billion dbcs ought to be enough for anybody! -- danda 2021.
        1000000000
    }

    pub fn unit_max() -> i8 {
        i8::MAX - 9 // this prevents some add/sub edge cases when unit is
                    // near i8::MAX and count is multi-digit.
                    // todo: revisit.
    }

    pub fn unit_min() -> i8 {
        -Self::unit_max()
    }

    fn to_rational(self) -> Rational {
        Rational::from(10).pow(self.unit as i32) * Rational::from(self.count)
    }

    pub fn to_si_string(self) -> String {
        let map: BTreeMap<i8, &str> = [
            (24, "yotta"),
            (21, "zetta"),
            (18, "exa"),
            (15, "peta"),
            (12, "tera"),
            (9, "giga"),
            (6, "mega"),
            (3, "kilo"),
            (2, "hecto"),
            (1, "deka"),
            (0, ""),
            (-1, "deci"),
            (-2, "centi"),
            (-3, "milli"),
            (-6, "micro"),
            (-9, "nano"),
            (-12, "pico"),
            (-15, "femto"),
            (-18, "atto"),
            (-21, "zepto"),
            (-24, "yocto"),
        ]
        .iter()
        .cloned()
        .collect();

        if self.unit >= -24 && self.unit <= 24 {
            let mut unit = self.unit;
            loop {
                if let Some(name) = map.get(&unit) {
                    let diff = self.unit.abs() - unit.abs();
                    let udiff = Integer::from(10).pow(diff as u32);
                    let newcount = Integer::from(self.count) * udiff;
                    return format!("{} {}", newcount, name);
                } else {
                    unit += if self.unit >= 0 { -1 } else { 1 };
                }
            }
        }

        self.to_string()
    }

    pub fn max() -> Self {
        Self {
            count: Self::counter_max(),
            unit: Self::unit_max(),
        }
    }

    // creates a normalized Amount from an Amount.
    //
    // todo: perhaps the normalized amount should always be instantiated
    //       with calling ::to_highest_unit() first.  Or maybe all Amount
    //       should be also. might just be extra work when not required though.
    fn to_normalized(self) -> NormalizedAmount {
        NormalizedAmount {
            count: Integer::from(self.count),
            unit: self.unit,
        }
    }

    // we may have an Amount like:
    // count = 25000,  unit = 2             (value: 2500000)
    //
    // We want instead an equivalent Amount:
    // count = 25,     unit = 5             (value: 2500000).
    //
    // This function turns the former into the latter.
    fn to_highest_unit(self) -> Self {
        let mut count = self.count;
        let mut unit = self.unit;
        while count % 10 == 0 && unit < Self::unit_max() {
            unit += 1;
            count = count.div_ceil(10);
        }
        Self::new(count, unit)
    }

    // we want to normalize these:
    // count = 25,  unit = 2    = 2500
    // count = 255, unit = 1    = 2550.

    // option a:
    // count = 25, unit = 2    = 25 * 100 = 2500
    // count = 25, unit = 2    = 25 * 10 = 2500    <---- loses information.

    // option b:
    // count = 250,  unit = 1    = 2500  <--- works.  but count can overflow.
    // count = 255,  unit = 1    = 2550.

    fn normalize(a: Self, b: Self) -> (NormalizedAmount, NormalizedAmount) {
        let a = a.to_highest_unit();
        let b = b.to_highest_unit();

        if a.unit == b.unit {
            (a.to_normalized(), b.to_normalized())
        } else if b.count == 0 {
            (
                a.to_normalized(),
                NormalizedAmount {
                    count: Integer::from(0),
                    unit: a.unit,
                },
            )
        } else if a.count == 0 {
            (
                NormalizedAmount {
                    count: Integer::from(0),
                    unit: b.unit,
                },
                b.to_normalized(),
            )
        } else {
            let unit_distance = if a.unit < b.unit {
                (a.unit..b.unit).len() as u32
            } else {
                (b.unit..a.unit).len() as u32
            };
            let unit_base = *[a.unit, b.unit].iter().min().unwrap();

            // println!("unit_distance_range: {:?}", (a.unit..b.unit));
            // println!("unit_distance: {}", unit_distance);

            let mut pair: Vec<NormalizedAmount> = [a, b]
                .iter()
                .rev()
                .map(|v| {
                    let count = if v.unit == unit_base {
                        Integer::from(v.count)
                    } else {
                        Integer::from(10).pow(unit_distance) * v.count
                    };
                    NormalizedAmount {
                        count,
                        unit: unit_base,
                    }
                })
                .collect();

            (pair.pop().unwrap(), pair.pop().unwrap())
        }
    }

    pub fn checked_add(self, other: Self) -> Result<Self> {
        // steps:
        // 1. normalize to same units.  use rug:Integer to represent count.
        // 2. add counts.
        // 3. find unit in which count is less than Self::counter_max()
        // 4. Amount::new()

        let (a, b) = Self::normalize(self, other);
        // println!("a: {:?}, b: {:?}", a, b);

        let mut count_sum = a.count + b.count;
        let mut unit = a.unit;
        if count_sum > 0 {
            while count_sum > Self::counter_max() || count_sum.clone() % 10 == 0 {
                unit += 1;
                count_sum = count_sum.div_ceil(10);
            }
        }

        match AmountCounter::try_from(count_sum) {
            Ok(v) if v <= Self::counter_max() => Ok(Amount::new(v, unit)),
            _ => Err(Error::AmountIncompatible),
        }
    }

    pub fn checked_sub(self, rhs: Self) -> Result<Self> {
        // we do not support negative Amounts
        if self < rhs {
            return Err(Error::AmountUnderflow);
        }

        // steps:
        // 1. normalize to same units.  use rug:Integer to represent count.
        // 2. subtract count.
        // 3. find unit in which count is less than Self::counter_max()
        // 4. Amount::new()

        // println!("-- sub() --");
        // println!("self: {:?}, other: {:?}", self, other);

        let (a, b) = Self::normalize(self, rhs);
        println!("a: {:?}, b: {:?}", a, b);

        let count_diff = a.count - b.count;
        println!("count_diff: {}", count_diff);

        match AmountCounter::try_from(count_diff) {
            Ok(v) if v <= Self::counter_max() => Ok(Amount::new(v, a.unit)),
            _ => Err(Error::AmountIncompatible),
        }
    }

    pub fn checked_sum<I>(iter: I) -> Result<Self>
    where
        I: Iterator<Item = Self>,
    {
        let mut sum = Amount::default();
        for v in iter {
            sum = sum.checked_add(v)?;
        }
        Ok(sum)

        // iter.fold(Some(Amount::default()), |a, b| a.checked_add(b))

        // this should be obsolete/slower than above now.
        // let mut r_sum = Rational::default();
        // for v in iter {
        //     r_sum = r_sum + v.to_rational();
        // }
        // Self::try_from(r_sum).unwrap()
    }
}

impl fmt::Display for Amount {
    // note:  this also creates ::to_string()
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}*10^{}", self.count, self.unit)
    }
}

// for a given number 234523 returns vec![2,3,4,5,2,3]
// todo: use an iterative impl instead of recursion.
pub(crate) fn digits(n: AmountCounter) -> Vec<u8> {
    fn x_inner(n: AmountCounter, xs: &mut Vec<u8>) {
        if n >= 10 {
            x_inner(n / 10, xs);
        }
        xs.push((n % 10) as u8);
    }
    let mut xs = Vec::new();
    x_inner(n, &mut xs);
    xs
}

impl PartialEq for Amount {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Amount {}

impl Hash for Amount {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // the following must hold true: k1 == k2 ⇒ hash(k1) == hash(k2)
        // todo: re-implement without to_rational(), which is slow.
        let r = self.to_rational();
        r.hash(state)
    }
}

impl Ord for Amount {
    // We perform the comparison without calculating exponent, which could be
    // very large.  Converting to Rational also works, but is slower.
    // Doubtless this could be optimized much further.
    fn cmp(&self, other: &Self) -> Ordering {
        let use_rational_impl = false;

        // note: converting to rationals is slower than our custom code below.
        if use_rational_impl {
            return self.to_rational().cmp(&other.to_rational());
        }

        match self.count {
            0 if other.count != 0 => return Ordering::Less,
            0 if other.count == 0 => return Ordering::Equal,
            _ if other.count == 0 => return Ordering::Greater,
            _ => {}
        }

        if self.unit == other.unit {
            return self.count.cmp(&other.count);
        }

        // a: Amount { count: 634438561, unit: 7 },
        // b: Amount { count: 486552,    unit: 10 }    <--- b is lesser

        let a_digits = digits(self.count);
        let b_digits = digits(other.count);

        // println!("a_unit: {}, a_digits_len: {}", self.unit, a_digits.len());
        // println!("b_unit: {}, b_digits_len: {}", other.unit, b_digits.len());
        let a_num_digits = self.unit as isize + a_digits.len() as isize;
        let b_num_digits = other.unit as isize + b_digits.len() as isize;

        // println!("a_num_digits: {}", a_num_digits);
        // println!("b_num_digits: {}", b_num_digits);

        if a_num_digits == b_num_digits {
            for (ad, bd) in a_digits.iter().zip(b_digits.iter()) {
                if ad > bd {
                    return Ordering::Greater;
                }
                if ad < bd {
                    return Ordering::Less;
                }
            }
            if a_digits.len() > b_digits.len() && a_digits[b_digits.len()..].iter().any(|d| *d > 0)
            {
                return Ordering::Greater;
            }
            if a_digits.len() < b_digits.len() && b_digits[a_digits.len()..].iter().any(|d| *d > 0)
            {
                return Ordering::Less;
            }
            Ordering::Equal
        } else {
            a_num_digits.cmp(&b_num_digits)
        }
    }
}

use quickcheck::{Arbitrary, Gen};

impl Arbitrary for Amount {
    fn arbitrary(g: &mut Gen) -> Self {
        let count = loop {
            let c = AmountCounter::arbitrary(g);
            if c <= Amount::counter_max() {
                break c;
            }
        };
        let unit = loop {
            let c = PowerOfTen::arbitrary(g);
            if c >= Amount::unit_min() && c <= Amount::unit_max() {
                break c;
            }
        };
        Self::new(count, unit)
    }
}

impl PartialOrd for Amount {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::Amount;
    use crate::{Error, Result};
    use quickcheck_macros::quickcheck;

    #[quickcheck]
    fn prop_hash_eq(a: Amount, b: Amount) -> Result<()> {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::Hash;
        use std::hash::Hasher;

        let mut ha = DefaultHasher::new();
        let mut hb = DefaultHasher::new();
        a.hash(&mut ha);
        b.hash(&mut hb);

        if a == b {
            assert_eq!(ha.finish(), hb.finish())
        } else {
            assert_ne!(ha.finish(), hb.finish())
        }

        Ok(())
    }

    #[quickcheck]
    fn amount_checked_sub(a: Amount, b: Amount) -> Result<()> {
        let result = a.checked_sub(b);

        match result {
            Ok(diff) => println!("{:?} - {:?} --> {:?}", a, b, diff),
            Err(Error::AmountUnderflow) => assert!(a < b),
            Err(Error::AmountIncompatible) => {
                println!("{:?} - {:?} --> Incompatible", a, b);
            }
            Err(e) => return Err(e),
        }
        Ok(())
    }

    #[quickcheck]
    fn prop_amount_checked_add(a: Amount, b: Amount) -> Result<()> {
        let result = a.checked_add(b);

        match result {
            Ok(sum) => println!("{:?} - {:?} --> {:?}", a, b, sum),
            Err(Error::AmountIncompatible) => {
                println!("{:?} - {:?} --> Incompatible", a, b);
            }
            Err(e) => return Err(e),
        }
        Ok(())
    }

    #[quickcheck]
    fn amount_sort(mut amounts: Vec<Amount>) -> Result<()> {
        amounts.sort();

        let mut iter = amounts.iter().peekable();
        loop {
            let cur = iter.next();
            let nxt = iter.peek();
            match (cur, nxt) {
                (Some(a), Some(b)) => {
                    println!("a: {:?}, b: {:?}", a, b);
                    assert!(a <= b);
                    assert!(a.to_rational() <= b.to_rational());
                }
                _ => break,
            }
        }

        Ok(())
    }

    #[quickcheck]
    fn prop_ord(amounts: Vec<(Amount, Amount)>) -> Result<()> {
        for (a, b) in amounts.iter() {
            if a > b {
                assert!(a.to_rational() > b.to_rational())
            } else if a < b {
                assert!(a.to_rational() < b.to_rational())
            } else {
                assert!(a.to_rational() == b.to_rational())
            }
        }
        Ok(())
    }

    #[quickcheck]
    fn prop_to_si_string(mut amounts: Vec<Amount>) -> Result<()> {
        amounts.sort();

        for a in amounts.into_iter() {
            if a.unit >= 0 && a.unit <= 24 && a.count > 0 {
                println!("{} \t\t<----- {:?}", a.to_si_string(), a);
            }
        }

        Ok(())
    }

    #[test]
    fn to_si_string_vector() -> Result<()> {
        let vector = vec![
            "1*10^-30",
            "1*10^-29",
            "1*10^-28",
            "1*10^-27",
            "1*10^-26",
            "1*10^-25",
            "1 yocto",
            "100 zepto",
            "10 zepto",
            "1 zepto",
            "100 atto",
            "10 atto",
            "1 atto",
            "100 femto",
            "10 femto",
            "1 femto",
            "100 pico",
            "10 pico",
            "1 pico",
            "100 nano",
            "10 nano",
            "1 nano",
            "100 micro",
            "10 micro",
            "1 micro",
            "100 milli",
            "10 milli",
            "1 milli",
            "1 centi",
            "1 deci",
            "1 ",
            "1 deka",
            "1 hecto",
            "1 kilo",
            "10 kilo",
            "100 kilo",
            "1 mega",
            "10 mega",
            "100 mega",
            "1 giga",
            "10 giga",
            "100 giga",
            "1 tera",
            "10 tera",
            "100 tera",
            "1 peta",
            "10 peta",
            "100 peta",
            "1 exa",
            "10 exa",
            "100 exa",
            "1 zetta",
            "10 zetta",
            "100 zetta",
            "1 yotta",
            "1*10^25",
            "1*10^26",
            "1*10^27",
            "1*10^28",
            "1*10^29",
        ];

        // note: to keep the fn shorter we only test range -30..30, rather than
        // -127..127
        for (idx, i) in (-30..30i8).enumerate() {
            let a = Amount::new(1, i);
            let strval = a.to_si_string();
            println!("{:?}\t--> {}", a, strval);
            assert_eq!(strval, vector[idx]);
        }

        Ok(())
    }
}
