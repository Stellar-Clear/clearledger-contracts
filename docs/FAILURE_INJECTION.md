# Escrow transfer failure coverage

Escrow transfer boundaries are covered with a strict test token in
`contracts/escrow/src/test.rs`. Unlike the permissive test token, the strict
token rejects transfers that would overdraw the escrow account. This exercises
failure handling without a network or a real Stellar asset.

Failure tests should assert the transfer error, preservation of the locked record,
and absence of a release event or history entry. Run the focused suite with:

```sh
cargo test -p clearledger-escrow
```

The fixture uses synthetic addresses and balances only.
