use crate::math::add;

pub fn fmt(a: i64, b: i64) -> String { format!("{}", add(a, b)) }

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn fmt_async_style() { assert_eq!(crate::fmt::fmt(1, 2), "3"); }
}
