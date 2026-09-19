# Changelog

All notable changes to this project are documented here.

## [0.4.0](https://github.com/seq-rs/yaml0/-/releases/tag/v0.4.0) - 2026-09-19


### Security
- **(code scanning alert)** Minimum permission specification in ci.yml ([`addd7e2`](https://github.com/seq-rs/yaml0/-/commit/addd7e2a7efa2ae8a9f4f0046bdd654a3bc68d75))


### Features
- Editing tests + lazygit fixture ([`8573e02`](https://github.com/seq-rs/yaml0/-/commit/8573e0242c55dcf27201f09017b9c5a51ed04d65))
- DocumentView + Document wrapper for reading and editing YAML streams, unit tests to validate ([`ed686bf`](https://github.com/seq-rs/yaml0/-/commit/ed686bf6244b8b5e3bb01f3188b4466048d1805f))
- In-place editing of YAML streams based on paths at runtime or compile-time ([`8e3dd61`](https://github.com/seq-rs/yaml0/-/commit/8e3dd611188d1b9aed8a18191809d8d17f2a8c0e))
- Span recording at parse-time ([`7d47bf9`](https://github.com/seq-rs/yaml0/-/commit/7d47bf9bf721851346718b4e8e6871ae00ccd2d6))
- Path search in parsed YAML stream ([`eea7fba`](https://github.com/seq-rs/yaml0/-/commit/eea7fbae40bdd4a73c6cd009adbeabd27d20e48b))


### Fixes
- Exports ([`63918d8`](https://github.com/seq-rs/yaml0/-/commit/63918d879ea109f1489dd66633e8dace6d77a381))


### Release process
- Prepare for the "edit" feature ([`167c761`](https://github.com/seq-rs/yaml0/-/commit/167c761830cd98ea4fc079a38e5b71620db67aa2))

## [0.3.0](https://github.com/seq-rs/yaml0/-/releases/tag/v0.3.0) - 2026-08-29


### Features
- **(emitter)** Flow nodes as implicit map keys ([`b16256a`](https://github.com/seq-rs/yaml0/-/commit/b16256a9edd555ae02c5c8be98aeea5acfd2e224))
- **(parser)** Flow nodes as implicit map keys ([`b7981ff`](https://github.com/seq-rs/yaml0/-/commit/b7981fff98da9efcbcc1c6229bc662d7292cc956))
- Explicit key handling ([`da93980`](https://github.com/seq-rs/yaml0/-/commit/da939809b15ffedbf4d0b68810940b5db95ef1e8))


### Fixes
- **(parser)** Reject multi-line implicit keys ([`86a2bea`](https://github.com/seq-rs/yaml0/-/commit/86a2bea3eda6cf6b49c0c7dfc393e4f4dfba0f1b))
- **(parser)** Multi-line plain scalars and document boundaries ([`59db15e`](https://github.com/seq-rs/yaml0/-/commit/59db15e76a59c5bf5ae8d80fe365b5cf831704f7))


### Release process
- **(actions)** Use seq-rs/actions instead of rust-cache and release ([`23352d6`](https://github.com/seq-rs/yaml0/-/commit/23352d67eeb975abd03035e2b19c751b62a800b2))

## [0.2.0](https://github.com/seq-rs/yaml0/-/releases/tag/v0.2.0) - 2026-06-10


### Features
- Token auth for initial publish ([`c71407b`](https://github.com/seq-rs/yaml0/-/commit/c71407bd038bd09763b8e17921e26314eee205b7))


### Improvements
- **(lib)** Rename library to `yaml0` (about time, right?) ([`554ddc0`](https://github.com/seq-rs/yaml0/-/commit/554ddc00f13eb4507198230b58e458b82a50ad5f))
- **(lib)** Add owned `Value`, rename underlying to `BorrowedValue` ([`523561f`](https://github.com/seq-rs/yaml0/-/commit/523561f2cc3021b24fa459187bdc7b96a68b1387))

## [0.1.3](https://github.com/seq-rs/yaml0/-/releases/tag/v0.1.3) - 2026-06-09


### Features
- **(Value)** Owned `Value` to simplify usage (under the hood, works with 0-copy BorrowedValue) ([`6945046`](https://github.com/seq-rs/yaml0/-/commit/69450468d0b861a678133a15dddea012b074e93a))


### Fixes
- Title ([`deab205`](https://github.com/seq-rs/yaml0/-/commit/deab2058bf6c54797f376ea49a236b65cecd05f7))


### Improvements
- **(Value)** Renamed to BorrowedValue to allow exporting owned 'Value' ([`efe65b9`](https://github.com/seq-rs/yaml0/-/commit/efe65b9582bb91c13f7a09f44b7d0e49a5dbb7ec))

## [0.1.2](https://github.com/seq-rs/yaml0/-/releases/tag/v0.1.2) - 2026-05-18


### Fixes
- Newline issue with publishing ([`6a3275f`](https://github.com/seq-rs/yaml0/-/commit/6a3275f4ac5bbd104019f13fe25ca7d7ee82aa89))

## [0.1.0](https://github.com/seq-rs/yaml0/-/releases/tag/v0.1.0) - 2026-05-18


