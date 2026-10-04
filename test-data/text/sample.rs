//! Highlighting sample.
fn main() {
    let xs: Vec<u32> = (1..=5).collect();
    println!("{:?}", xs.iter().map(|x| x * x).sum::<u32>());
}
