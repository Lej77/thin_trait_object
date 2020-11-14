//! Some experiments to see what kinds of APIs would be possible for this
//! crate. Also some testing for what Rust allows for traits that are
//! object safe.

/// Check if it is possible to construct a type via a type alias.
#[test]
#[allow(dead_code, clippy::no_effect)]
fn type_alias_builder() {
    trait GetBuilder {
        type Builder;
    }
    struct Builder {
        a: u32,
        b: bool,
    }
    impl GetBuilder for () {
        type Builder = Builder;
    }
    type ABuilder<T> = <T as GetBuilder>::Builder;

    ABuilder::<()> { a: 1, b: false };
}

/// Check if a builder that takes closures can be const.
#[test]
#[allow(dead_code)]
fn const_builder() {
    fn v() {}
    const V: fn() = v;

    struct Builder {
        a: u32,
    }
    impl Builder {
        const fn new() -> Self {
            Self { a: 0 }
        }
        const fn a(mut self, a: u32) -> Self {
            self.a = a;
            self
        }
    }
    const B: Builder = Builder::new().a(2);

    /*
    struct Builder2 {
        a: fn(),
    }
    impl Builder2 {
        const fn new() -> Self {
            Self {
                a: v,
            }
        }
        const fn a(mut self, a: fn()) -> Self {
            self.a = a;
            self
        }
    }
    */

    struct Builder3<F> {
        a: F,
    }
    impl Builder3<()> {
        const fn new() -> Self {
            Self { a: () }
        }
        const fn a<F>(self, a: F) -> Builder3<F> {
            Builder3::<F> { a }
        }
    }
    const B3: Builder3<fn()> = Builder3::new().a(v);
    // */
}

/// Check if we can use auto trait on a trait as a convenient API for specifying
/// allowed auto traits.
///
/// https://doc.rust-lang.org/reference/special-types-and-traits.html#auto-traits
#[test]
fn dyn_trait_for_auto_trait_info() {
    trait VTableTrait {}
    trait GetAutoTraitInfo {}

    // All these trait impls would need to be generated for each new vtable
    // trait in order to provide convenient access to auto trait options.
    ////////////////////////////////////////////////////////////////////////////////
    // Trait impls for core auto traits
    ////////////////////////////////////////////////////////////////////////////////

    impl GetAutoTraitInfo for dyn VTableTrait {}

    impl GetAutoTraitInfo for dyn VTableTrait + Send {}
    impl GetAutoTraitInfo for dyn VTableTrait + Sync {}
    impl GetAutoTraitInfo for dyn VTableTrait + Unpin {}

    impl GetAutoTraitInfo for dyn VTableTrait + Send + Sync {}
    // Fortunately, order doesn't matter:
    // impl GetAutoTraitInfo for dyn VTableTrait + Sync + Send  {}
    impl GetAutoTraitInfo for dyn VTableTrait + Send + Unpin {}
    impl GetAutoTraitInfo for dyn VTableTrait + Sync + Unpin {}

    impl GetAutoTraitInfo for dyn VTableTrait + Send + Sync + Unpin {}

    ////////////////////////////////////////////////////////////////////////////////
    // Trait impls for std only auto traits (These are less useful so we could skip them)
    ////////////////////////////////////////////////////////////////////////////////

    impl GetAutoTraitInfo for dyn VTableTrait + std::panic::UnwindSafe {}
    impl GetAutoTraitInfo for dyn VTableTrait + std::panic::RefUnwindSafe {}

    impl GetAutoTraitInfo for dyn VTableTrait + std::panic::UnwindSafe + Send {}
    impl GetAutoTraitInfo for dyn VTableTrait + std::panic::UnwindSafe + Sync {}
    impl GetAutoTraitInfo for dyn VTableTrait + std::panic::UnwindSafe + Unpin {}
    impl GetAutoTraitInfo for dyn VTableTrait + std::panic::UnwindSafe + std::panic::RefUnwindSafe {}
    impl GetAutoTraitInfo for dyn VTableTrait + std::panic::RefUnwindSafe + Send {}
    impl GetAutoTraitInfo for dyn VTableTrait + std::panic::RefUnwindSafe + Sync {}
    impl GetAutoTraitInfo for dyn VTableTrait + std::panic::RefUnwindSafe + Unpin {}

    impl GetAutoTraitInfo for dyn VTableTrait + std::panic::UnwindSafe + Send + Sync {}
    impl GetAutoTraitInfo for dyn VTableTrait + std::panic::UnwindSafe + Send + Unpin {}
    impl GetAutoTraitInfo
        for dyn VTableTrait + std::panic::UnwindSafe + Send + std::panic::RefUnwindSafe
    {
    }
    impl GetAutoTraitInfo for dyn VTableTrait + std::panic::UnwindSafe + Sync + Unpin {}
    impl GetAutoTraitInfo
        for dyn VTableTrait + std::panic::UnwindSafe + Sync + std::panic::RefUnwindSafe
    {
    }
    impl GetAutoTraitInfo for dyn VTableTrait + std::panic::RefUnwindSafe + Send + Sync {}
    impl GetAutoTraitInfo for dyn VTableTrait + std::panic::RefUnwindSafe + Send + Unpin {}
    impl GetAutoTraitInfo for dyn VTableTrait + std::panic::RefUnwindSafe + Sync + Unpin {}

    impl GetAutoTraitInfo for dyn VTableTrait + std::panic::UnwindSafe + Send + Sync + Unpin {}
    impl GetAutoTraitInfo
        for dyn VTableTrait + std::panic::UnwindSafe + Send + Sync + std::panic::RefUnwindSafe
    {
    }
    impl GetAutoTraitInfo
        for dyn VTableTrait + std::panic::UnwindSafe + Sync + Unpin + std::panic::RefUnwindSafe
    {
    }
    impl GetAutoTraitInfo for dyn VTableTrait + std::panic::RefUnwindSafe + Send + Sync + Unpin {}

    // All auto traits:
    impl GetAutoTraitInfo
        for dyn VTableTrait
            + Send
            + Sync
            + Unpin
            + std::panic::UnwindSafe
            + std::panic::RefUnwindSafe
    {
    }

    ////////////////////////////////////////////////////////////////////////////////
    // Other ways of going from dyn Trait + auto traits to types with info
    ////////////////////////////////////////////////////////////////////////////////

    // Can check if a type implements an auto trait:
    fn take_send<T: Send + ?Sized>() {}
    take_send::<dyn VTableTrait + Send>();
    //take_send::<dyn VTableTrait>();

    // If the trait has an auto trait as a super trait then the trait object will
    // implement the auto trait.
    trait SendBound: Send {}
    take_send::<dyn SendBound>();

    // Can add auto traits that are already enforced.
    impl VTableTrait for dyn SendBound {}
    impl VTableTrait for dyn SendBound + Send {}
    take_send::<dyn SendBound + Send>();

    // take_send::<*mut ()>();

    #[allow(dead_code)]
    fn take_unpin<T: Unpin + ?Sized>() {}
    // take_unpin::<dyn VTableTrait>();
    // take_unpin::<*mut ()>();

    #[allow(dead_code)]
    fn take_unwind_safe<T: std::panic::UnwindSafe + ?Sized>() {}
    // take_unwind_safe::<dyn VTableTrait>();
    // take_unwind_safe::<*mut ()>();

    enum Never {}
    take_send::<Never>();
    take_unpin::<Never>();
    take_unwind_safe::<Never>();
}

/// An API that specifies what auto traits should be used for a vtable via
/// a const instance of a config struct.
#[test]
fn const_auto_trait_config() {
    struct AutoTraitConfig {
        send: bool,
        sync: bool,
    }
    impl AutoTraitConfig {
        const fn encode_info(self) -> usize {
            (self.send as usize) << 1 | (self.sync as usize)
        }
    }
    trait GetConfig {
        const CONFIG: AutoTraitConfig;
    }
    #[allow(dead_code)]
    struct SomeVTable;
    impl GetConfig for SomeVTable {
        const CONFIG: AutoTraitConfig = AutoTraitConfig {
            send: false,
            sync: false,
        };
    }
    trait ConfigAsType: GetConfig {
        type ConfigType;
    }
    impl<T> ConfigAsType for T
    where
        T: GetConfig,
    {
        type ConfigType = [(); {
            // Currently can't convert a const to a type via traits. So we would
            // need to convert each config struct to a type manually.
            // <T as GetConfig>::CONFIG.encode_info()
            0
        }];
    }

    trait ManuallyConfigAsType {
        type ConfigType;
    }
    macro_rules! specify_config {
        ($vtable_type:ty, $config:expr) => {
            impl ManuallyConfigAsType for $vtable_type {
                type ConfigType = [(); {
                    let config: AutoTraitConfig = $config;
                    config.encode_info()
                }];
            }
        };
    }
    specify_config!(
        SomeVTable,
        AutoTraitConfig {
            send: false,
            sync: true,
        }
    );

    println!(
        "{:b}",
        AutoTraitConfig {
            send: true,
            sync: true
        }
        .encode_info()
    );
}

#[cfg(compiler_error_in_the_future)]
#[test]
fn where_clause_on_trait_object_methods() {
    // This will be a compiler error in the future:
    trait SomeTrait {
        fn test(&self)
        where
            Self: Send;
        fn test2(&self)
        where
            Self: std::fmt::Debug;
    }
    impl SomeTrait for () {
        fn test(&self)
        where
            Self: Send,
        {
        }
        fn test2(&self)
        where
            Self: std::fmt::Debug,
        {
        }
    }
    trait GenericTrait<T> {
        // This seems to be allowed:
        fn test(&self)
        where
            T: Send;
    }
    impl GenericTrait<()> for () {
        fn test(&self) {}
    }
    let _a: &(dyn GenericTrait<()> + Send) = &();

    trait SubTrait: SomeTrait + std::fmt::Debug {}
    impl<T> SubTrait for T where T: SomeTrait + std::fmt::Debug {}

    let _a: &dyn SomeTrait = &();
    //a.test();
    let a: &(dyn SomeTrait + Send) = &();
    a.test();
    // a.test2();

    let a: &dyn SubTrait = &();
    a.test2();
}

#[test]
fn trait_object_with_consuming_methods() {
    trait WWW {
        fn consume(self);

        fn other(value: u32)
        where
            Self: Sized;
    }
    impl WWW for () {
        fn consume(self) {}

        fn other(_value: u32)
        where
            Self: Sized,
        {
        }
    }

    let _w: Box<dyn WWW> = Box::new(());
    // _w.consume();
}

/// Can we have a type with lifetimes that is static promoted.
#[test]
#[allow(unused_mut, unused_variables)]
fn static_lifetime() {
    struct WithLife<'a>(fn(&'a mut u32));
    let vtable: &'static WithLife<'_> = &WithLife(|w| {
        *w = 2;
    });
    let mut value = 2;
    // Nope: value reference must be 'static here:
    // (vtable.0)(&mut value);
}

/// See if miri warns about using pointer arithmetic to access a location
/// based on a reference's value even though that location isn't covered
/// by the reference.
///
/// Seems to work fine!
#[test]
fn go_to_field_of_parent_struct() {
    #[repr(C)]
    struct Foo {
        a: u32,
        b: u32,
    }

    let a =
        unsafe { &mut *((Box::into_raw(Box::new(Foo { a: 2, b: 10 })) as *mut Foo) as *mut u32) };
    assert_eq!(*a, 2);
    *a += 20;
    assert_eq!(*a, 22);

    let b = unsafe { &mut *((a as *mut u32).wrapping_add(1)) };
    assert_eq!(*b, 10);
    *b += 2;
    assert_eq!(*b, 12);
    unsafe { Box::from_raw((a as *mut _) as *mut Foo) };
}

/// Check if we can shorten the lifetime of a trait object if we are using an
/// associated type based on it.
///
/// Conclusion: this does not seem possible.
#[test]
#[allow(dead_code)]
fn lifetime_variance_with_associated_type() {
    use std::marker::PhantomData;

    trait GetAssocType {
        type Assoc;
    }
    impl<'a> GetAssocType for dyn ToString + 'a {
        type Assoc = &'a ();
    }

    struct WithAssoc<T: GetAssocType + ?Sized> {
        // If this is removed then the lifetime can be shortened but otherwise it can't.
        marker: PhantomData<T::Assoc>,
        t_marker: PhantomData<T>,
    }

    struct WithAssocAndLifetime<'a, T: GetAssocType<Assoc = &'a ()> + ?Sized> {
        marker: PhantomData<T::Assoc>,
        t_marker: PhantomData<T>,
    }

    fn shorten_box<'a>(
        long: Box<dyn ToString + 'static>,
        _short: &'a (),
    ) -> Box<dyn ToString + 'a> {
        long
    }

    fn shorten_assoc<'a>(
        _long: WithAssoc<dyn ToString + 'static>,
        _short: &'a (),
    ) -> WithAssoc<dyn ToString + 'a> {
        // return _long;
        todo!();
    }

    fn shorten_assoc_with_lifetime<'a>(
        _long: WithAssocAndLifetime<'static, dyn ToString + 'static>,
        _short: &'a (),
    ) -> WithAssocAndLifetime<'a, dyn ToString + 'a> {
        // return _long;
        todo!();
    }
}
