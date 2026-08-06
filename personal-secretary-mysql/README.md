# Personal Secretary MySQL Adapter

This crate implements Personal Secretary application ports with SeaORM and MySQL. It owns database
entities, SQL, transaction boundaries, persistence error mapping, and MySQL integration tests.

Dependency direction:

```text
personal-secretary-mysql -> personal-secretary
qqbot-server -> personal-secretary + personal-secretary-mysql
```

`personal-secretary` must never depend back on this crate.
