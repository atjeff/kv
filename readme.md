# kv

## What
- This is a key-value store exposed via an Axum HTTP API and persisted on disk via [meilisearch/heed](https://github.com/meilisearch/heed).

## Why
- I wanted to learn more about Rust and Axum, so I decided to build a simple key-value store.

## Usage
- You can use it by running `cargo run` in the root directory of the project. This will start the server at `localhost:3000`.

## Configuration
- You can configure the server by setting the following environment variables:
    - `DB_PATH`: The directory to store the data in. Defaults to `./db/heed.mdb`.

## Backup / Restore
- You can backup the data by copying the `DB_PATH` directory.
- You can restore the data by replacing the `DB_PATH` directory with the backup.
- This can easily be stored in S3/R2 blob storage.

# TODO
- [x] Add tests
- [x] Conform to proper REST API standards
- [ ] Add more documentation
- [x] Add more configuration options
- [x] Add more error handling
- [x] Add more logging
- [x] Add more metrics
- [ ] Add more security
- [ ] Add more validation

