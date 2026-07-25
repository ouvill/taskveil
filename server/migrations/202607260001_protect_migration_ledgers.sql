-- Migration bookkeeping is owner-only control-plane state. The broad
-- taskveil_app grants used for application tables must not include either
-- SQLx's ledger or the one-time zero-knowledge reset marker.
REVOKE ALL PRIVILEGES ON TABLE _sqlx_migrations FROM taskveil_app;
REVOKE ALL PRIVILEGES ON TABLE taskveil_schema_migrations FROM taskveil_app;
