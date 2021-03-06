-- This file was automatically created by Diesel to setup helper functions
-- and other internal bookkeeping. This file is safe to edit, any future
-- changes will be added to existing projects as new migrations.




-- Sets up a trigger for the given table to automatically set a column called
-- `updated_at` whenever the row is modified (unless `updated_at` was included
-- in the modified columns)
--
-- # Example
--
-- ```sql
-- CREATE TABLE users (id SERIAL PRIMARY KEY, updated_at TIMESTAMP NOT NULL DEFAULT NOW());
--
-- SELECT diesel_manage_updated_at('users');
-- ```
-- CREATE OR REPLACE FUNCTION diesel_manage_updated_at(_tbl regclass) RETURNS VOID AS $$
-- BEGIN
--    EXECUTE format('CREATE TRIGGER set_updated_at BEFORE UPDATE ON %s
--                    FOR EACH ROW EXECUTE PROCEDURE diesel_set_updated_at()', _tbl);
-- END;
-- $$ LANGUAGE plpgsql;

-- CREATE OR REPLACE FUNCTION diesel_set_updated_at() RETURNS trigger AS $$
-- BEGIN
--    IF (
--        NEW IS DISTINCT FROM OLD AND
--        NEW.updated_at IS NOT DISTINCT FROM OLD.updated_at
--    ) THEN
--        NEW.updated_at := current_timestamp;
--    END IF;
--    RETURN NEW;
-- END;
-- $$ LANGUAGE plpgsql;

CREATE TABLE IF NOT EXISTS bundle (
    id CHAR(43) NOT NULL,
    owner_address CHAR(43) NOT NULL,
    block_height BYTEA NOT NULL,
    PRIMARY KEY (id)
);

CREATE TABLE IF NOT EXISTS transactions (
    id CHAR(43) NOT NULL,
    epoch BYTEA NOT NULL,
    block_promised BYTEA NOT NULL,
    block_actual BYTEA,
    signature BYTEA NOT NULL,
    validated BOOLEAN NOT NULL,
    bundle_id CHAR(43),
    PRIMARY KEY (id),
    FOREIGN KEY (bundle_id) REFERENCES bundle(id)
);

CREATE TABLE IF NOT EXISTS validators (
    address CHAR(43) NOT NULL,
    url VARCHAR(100),
    PRIMARY KEY(address)
);

CREATE TABLE IF NOT EXISTS leaders (
    address CHAR(43) NOT NULL,
    PRIMARY KEY(address),
    FOREIGN KEY(address) REFERENCES validators(address)
);

CREATE INDEX epoch_transactions_idx ON transactions(epoch);
