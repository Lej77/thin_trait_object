mod api_experiments;

/// Tests that check if some code fails to compile.
#[test]
#[cfg(not(miri))]
fn compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile_fail/*.rs");
}

#[test]
fn it_works() {
    use super::*;

    //trace_macros!(false);
    define_v_table!(
        /// Test
        pub(super) trait TestMacroParsing<'a>: 'static + Send + Sync {
            type TestType: 'a + Clone + FnOnce(u32) -> i32;

            // fn ambiguous_with_type(self) -> Self::Test;
            fn with_type(self) -> <Self as TestMacroParsing<'a>>::TestType;

            //fn test<T>(&self);

            unsafe fn an_unsafe_method(self);

            fn method_ref(&self);

            fn method_ref_default(&self, arg1: u32, arg2: bool) -> bool {
                arg1 == 0 && arg2
            }

            #[allow(clippy::needless_arbitrary_self_type)]
            fn method_ref_adv(self: &Self);

            fn method_ref_adv_default(mut self: &Self) {
                let mut c = move || {
                    self = self;
                };
                c();
            }

            fn method_mut(&mut self);

            fn method_mut_default(&mut self) {}

            #[allow(clippy::needless_arbitrary_self_type)]
            fn method_mut_adv(self: &mut Self);

            #[allow(unused_mut)]
            fn method_mut_adv_default(mut self: &mut Self) {}

            fn method(self);

            #[allow(clippy::needless_arbitrary_self_type)]
            fn method_adv(self: Self);
        }
    );
    //trace_macros!(false);

    define_v_table!(
        trait TestVTable {
            fn is_equal(&self, number: u32) -> bool;
            fn set_value(&mut self, number: u32);
            fn clone(&self) -> ThinBox<dyn TestVTable, bool>;
            fn consume(self) -> u32;
        }
    );
    impl TestVTable for u32 {
        fn is_equal(&self, number: u32) -> bool {
            *self == number
        }
        fn set_value(&mut self, value: u32) {
            *self = value;
        }
        fn clone(&self) -> ThinBox<dyn TestVTable, bool> {
            ThinBox::new(*self, false)
        }
        fn consume(self) -> u32 {
            self
        }
    }

    // Test low level API (useful to narrow down miri issues):
    RawThinBox::<<dyn TestVTable as ThinTrait<_>>::VTable, _, _, _, _>::new(2, 3_u128)
        .with_auto_trait_config::<()>()
        .erase()
        .free_common_data()
        .free_via_vtable();

    ThinBox::<dyn TestVTable, _>::into_raw(ThinBox::from_raw(
        RawThinBox::<<dyn TestVTable as ThinTrait<_>>::VTable, _, _, _, _>::new(2, 3_u128)
            .with_auto_trait_config::<()>()
            .erase(),
    ))
    .free_common_data()
    .free_via_vtable();

    ThinBox::<dyn TestVTable, _>::new(2, 123_u128);

    // High level API:

    fn test_thin_box(mut erased: &mut ThinBox<dyn TestVTable, bool>) {
        assert_eq!(mem::size_of_val(&erased), mem::size_of::<usize>());

        // Check if trait impl for ThinBox works:
        test_callable::<ThinBox<_, _>>(&mut erased);

        assert!(erased.is_equal(2));
        assert!((&**erased).is_equal(2));
        assert!(!erased.is_equal(3));

        test_thin(&mut erased);
    }
    fn test_callable<T: TestVTable + ?Sized>(v: &mut T) {
        v.set_value(4);
        assert_eq!(v.clone().consume(), 4);
        assert!(v.is_equal(4));
        v.set_value(2);
    }
    fn test_thin(thin: &mut Thin<dyn TestVTable, bool>) {
        {
            let (erased, common) = Thin::split_common(thin);
            assert!(erased.is_equal(2));
            assert!(!common);
        }
        {
            let (erased, common) = Thin::split_common_mut(thin);
            test_callable::<ThinWithoutCommon<_, _>>(erased);
            *common = true;
        }
    }

    let mut erased = ThinBox::<dyn TestVTable, _>::new(2, false);
    test_thin_box(&mut erased);

    let (mut erased, common) = ThinBox::take_common(erased);
    assert!(common);
    test_callable::<ThinBoxWithoutCommon<_, _>>(&mut erased);
    test_callable::<ThinWithoutCommon<_, _>>(&mut *erased);
    assert!(erased.is_equal(2));
    assert!((&*erased).is_equal(2));
    assert_eq!(mem::size_of_val(&erased), mem::size_of::<usize>());

    let mut erased = ThinBoxWithoutCommon::put_common(erased, false);
    // Redo all tests to ensure nothing went wrong when converting types:
    test_thin_box(&mut erased);

    let mut owned = OwnedThin::<dyn TestVTable, _, _>::new(2, false);
    test_thin(&mut owned);
    assert!(!owned.is_equal(4));
    assert!((&**owned).is_equal(2));
    assert_eq!(owned.into_inner(), (2, true));

    // Ensure all drop impls work:
    drop(OwnedThin::<dyn TestVTable, _, _>::new(4, false));
    drop(ThinBox::<dyn TestVTable, _>::new(4, false));
    drop(ThinBoxWithoutCommon::<dyn TestVTable, _>::new(4));

    // Check that consuming methods work:
    assert_eq!(ThinBox::<dyn TestVTable, _>::new(5, false).consume(), 5);
    assert_eq!(
        ThinBoxWithoutCommon::<dyn TestVTable, _>::new(7).consume(),
        7
    );

    // Check if traits with lifetimes work:
    define_v_table!(
        trait WithLifetime<'a> {}
    );
    impl<'a, T> WithLifetime<'a> for T where T: ToString {}

    let text = "".to_owned();
    ThinBox::<dyn WithLifetime<'static>, _>::new(&text, ());
}

#[test]
#[allow(dead_code)]
fn lifetime_variance() {
    use super::*;
    define_v_table!(
        trait ToStringVTable {
            fn dyn_to_string(&self) -> String;
        }
    );

    impl<T> ToStringVTable for T
    where
        T: ToString + ?Sized,
    {
        fn dyn_to_string(&self) -> String {
            self.to_string()
        }
    }

    fn shorten_box<'a>(
        long: Box<dyn ToString + 'static>,
        _short: &'a (),
    ) -> Box<dyn ToString + 'a> {
        long
    }
    fn shorten_thin<'a>(
        long: ThinBox<dyn ToStringVTable + 'static, ()>,
        _short: &'a (),
    ) -> ThinBox<dyn ToStringVTable + 'a, ()> {
        // Returning long itself would fail:
        // return long;
        // Casting explicitly works though:
        // return ThinBox::from_raw(ThinBox::into_raw(long));
        ThinBox::shorten_lifetime(long)
    }
    fn lengthen_thin<'a>(
        _short: ThinBox<dyn ToStringVTable + 'a, ()>,
    ) -> ThinBox<dyn ToStringVTable + 'static, ()> {
        // Returning long itself would fail:
        // return _short;
        // This also fails:
        // return ThinBox::from_raw(ThinBox::into_raw(_short));
        todo!()
    }
}

#[test]
fn auto_traits() {
    use super::*;
    define_v_table!(
        trait SomeVTable {}
    );
    impl<T: Clone> SomeVTable for T {}

    // Simple way to name the marker type with compiler error:
    // fn _take_marker(_: &ThinTraitAutoTraitsMarker<dyn SomeVTable + Send, ()>) {}
    // _take_marker(&());

    assert!(impls::impls!(ThinTraitAutoTraitsMarker<dyn SomeVTable + Send, ()>: Send & !Sync));

    assert!(impls::impls!(ThinBox<dyn SomeVTable, ()>: !Send));
    assert!(impls::impls!(Thin<dyn SomeVTable, ()>: !Send));

    assert!(impls::impls!(ThinBox<dyn SomeVTable + Send, ()>: Send & !Sync));
    assert!(impls::impls!(ThinBox<dyn SomeVTable + Send, alloc::rc::Rc<()>>: !Send & !Sync));

    ThinBox::<dyn SomeVTable + Send, _>::new(2, ());
    // The next line shouldn't compile:
    //ThinBox::<dyn SomeVTable + Send, _>::new(alloc::rc::Rc::new(2), ());
}

#[test]
fn auto_traits_for_supertraits() {
    use super::*;
    define_v_table!(
        trait SomeVTableSend: Send {}
    );
    assert!(impls::impls!(ThinBox<dyn SomeVTableSend, ()>: Send));
    assert!(impls::impls!(Thin<dyn SomeVTableSend, ()>: Send));
    assert!(impls::impls!(Thin<dyn SomeVTableSend, alloc::rc::Rc<()>>: !Send & !Sync));
}
