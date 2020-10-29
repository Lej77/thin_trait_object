# thin_trait_object

An experiment to provide a safe API for storing trait objects using thin
pointers instead of fat pointers.

Inspired from code in the [`anyhow`](https://crates.io/crates/anyhow) crate.
Specifically [code from version 1.0.33](https://github.com/dtolnay/anyhow/tree/c25be95f1a24f7497d2b4530ffcb4f90d3871975).

Check out the [`prelude`] module for the most important items when using this
crate.

<!-- Generate README.md using `cargo readme --no-license > README.md` -->

## Examples

```rust
use thin_trait_object::{define_v_table, ThinBox, Thin};

use core::mem;

define_v_table!(
    trait TestCallable {
        fn is_equal(&self, number: u32) -> bool;
    }
);
impl TestCallable for u32 {
    fn is_equal(&self, number: u32) -> bool { *self == number }
}
let mut erased = ThinBox::<dyn TestCallable, _>::new(2, false);
assert_eq!(mem::size_of_val(&erased), mem::size_of::<usize>());

assert!(erased.is_equal(2));
assert!(!erased.is_equal(3));
{
    let (erased, common) = Thin::split_common(&erased);
    assert!(erased.is_equal(2));
    assert!(!common);
}
{
    let (erased, common) = Thin::split_common_mut(&mut erased);
    assert!(erased.is_equal(2));
    *common = true;
}
let (erased, common) = ThinBox::take_common(erased);
assert!(common);
assert!(erased.is_equal(2));
assert_eq!(mem::size_of_val(&erased), mem::size_of::<usize>());
```

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
