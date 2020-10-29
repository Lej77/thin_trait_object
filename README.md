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
    trait VTableEq {
        fn is_equal(&self, number: u32) -> bool;
    }
);
impl VTableEq for u32 {
    fn is_equal(&self, number: u32) -> bool { *self == number }
}

// While a Box<dyn Trait> has the size of 2 usize this is only 1 usize large:
assert_eq!(mem::size_of::<usize>(), mem::size_of::<ThinBox::<dyn VTableEq, bool>>());
assert_eq!(mem::size_of::<usize>() * 2, mem::size_of::<Box<dyn VTableEq>>());

// Need to specify the trait that the provided value implements (but the actual
// type is erased/forgotten):
let mut erased = ThinBox::<dyn VTableEq, bool>::new(2, false);

// ThinBox implements the `VTableEq` trait:
assert!(erased.is_equal(2));
assert!(!erased.is_equal(3));
// Can split a reference into the "common data" part and the type erased object
// part:
{
    let (erased, common) = Thin::split_common(&erased);
    assert!(erased.is_equal(2));
    assert!(!common);
}
// Can also split a mutable reference:
{
    let (erased, common) = Thin::split_common_mut(&mut erased);
    assert!(erased.is_equal(2));
    *common = true;
}
// Can move the "common data" out of the allocation and still continue using
// it to interact with the type erased object:
let (erased, common) = ThinBox::take_common(erased);
assert!(common);
assert!(erased.is_equal(2));
```

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
