A comprehensive library providing safe abstractions for Foreign Function
Interface (FFI) operations.  The `foreign` crate offers traits to safely
convert between Rust types and their C representations, providing clarity
about which operations allocate memory and therefore the performance
characteristics.

## Overview

When working with C libraries from Rust, different Rust types may
have different naming and conventions for converting to their
C representation and back.  Even if you try to keep a common
convention for your own `enum`s and `struct`s, they still wouldn't
apply to standard library `struct`s such as [`String`]/[`str`] and
[`CString`](std::ffi::CString)/[`CStr`](std::ffi::CStr).

This library provides a comprehensive set of traits and wrapper types
that abstract the lifetime of the resulting pointers and that make these
operations ergonomic.

The conversion traits themselves follow intuitive naming patterns that
clearly indicate their behavior and ownership implications.  Rust terms
like `clone` or `clone_from` are used to show clearly where allocations
happen.  [`FromForeign`] and its dual [`IntoNative`] convert C data
to Rust, and there are a variety of traits for different scenarios of
C&nbsp;→&nbsp;Rust conversion: [`CloneToForeign`] is the most generic
but performs a deep copy; [`IntoForeign`] consumes the Rust object;
[`BorrowForeign`] and [`BorrowForeignMut`] borrow the contents
of the Rust object and perform no allocation.  The performance
characteristics of each conversion are fixed: methods other than
[`clone_to_foreign()`](CloneToForeign::clone_to_foreign) are only
available if they can operate with no copying.

## Usage examples

### Rust → C Conversion

Copying:

```ignore
let foreign = rust_value.clone_to_foreign();
call_c_function(foreign.as_ptr());
// Value freed automatically
```

Giving ownership to C:

```ignore
let foreign_ptr = rust_value.clone_to_foreign_ptr();
call_c_function(foreign_ptr);
```

Consuming:

```ignore
// rust_value is typically heap-allocated, e.g. a Box, String or Vec
let foreign = rust_value.into_foreign();
call_c_function(foreign.as_ptr());
// Value freed automatically
```

Borrowing for temporary use:

```ignore
let rust_data = get_some_data();
let borrowed = rust_data.borrow_foreign();
call_c_function(borrowed.as_ptr());
```

### C → Rust Conversion

Copying:

```ignore
let rust_value = unsafe { RustType::cloned_from_foreign(c_ptr) };
```

Copying and freeing the C version:

```ignore
let rust_value = unsafe { RustType::from_foreign(c_ptr) };
```

```ignore
let rust_value: RustType = unsafe { c_ptr.into_native() };
```

## Comparison to `glib-rs`

glib-rs provides a similar set of traits.  A rough comparison is
as follows:

| `glib-rs`          | `foreign-rs`             |
|--------------------|--------------------------|
| `from_glib_full()` | [`from_foreign()`](FromForeign::from_foreign)         |
| `from_glib_none()` | [`cloned_from_foreign()`](FromForeign::cloned_from_foreign)  |
| `to_glib_full()`   | [`clone_to_foreign_ptr()`](CloneToForeign::clone_to_foreign_ptr) |
| `to_glib_none()`   | [`clone_to_foreign()`](CloneToForeign::clone_to_foreign)     |
