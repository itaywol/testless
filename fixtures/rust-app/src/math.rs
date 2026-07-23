pub fn add(a: i64, b: i64) -> i64 { a + b }

pub struct Calc { pub total: i64 }

impl Calc {
    pub fn push(&mut self, n: i64) { self.total = add(self.total, n); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_works() { assert_eq!(add(2, 2), 4); }

    #[test]
    fn calc_push() {
        let mut c = Calc { total: 0 };
        c.push(5);
        assert_eq!(c.total, 5);
    }
}
