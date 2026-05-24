// Package db provides database utilities for the Blazil payments service.
package db

import (
	"errors"
	"fmt"
	"strings"

	// embed is imported for its side-effect: populating the migrations FS.
	_ "embed"

	"embed"

	"github.com/golang-migrate/migrate/v4"
	_ "github.com/golang-migrate/migrate/v4/database/pgx/v5"
	"github.com/golang-migrate/migrate/v4/source/iofs"
	"go.uber.org/zap"
)

//go:embed migrations/*.sql
var migrationsFS embed.FS

// RunMigrations applies all pending up-migrations to the database at databaseURL.
// It is safe to call on every startup: if the schema is already current,
// migrate.ErrNoChange is treated as success and logged at debug level.
//
// A dirty migration version (previous run failed mid-flight) is treated as a
// fatal error — the caller should halt startup to prevent data corruption.
func RunMigrations(databaseURL string, logger *zap.Logger) error {
	src, err := iofs.New(migrationsFS, "migrations")
	if err != nil {
		return fmt.Errorf("load embedded migrations: %w", err)
	}

	m, err := migrate.NewWithSourceInstance("iofs", src, toPgx5URL(databaseURL))
	if err != nil {
		return fmt.Errorf("init migrator: %w", err)
	}
	defer m.Close()

	version, dirty, verErr := m.Version()
	if verErr != nil && !errors.Is(verErr, migrate.ErrNilVersion) {
		return fmt.Errorf("query schema version: %w", verErr)
	}
	if dirty {
		return fmt.Errorf("payments schema is dirty at version %d — manual intervention required", version)
	}
	if !errors.Is(verErr, migrate.ErrNilVersion) {
		logger.Info("payments schema version before migration", zap.Uint("version", version))
	}

	if err := m.Up(); err != nil {
		if errors.Is(err, migrate.ErrNoChange) {
			logger.Debug("payments schema already up-to-date")
			return nil
		}
		return fmt.Errorf("run migrations: %w", err)
	}

	version, _, _ = m.Version()
	logger.Info("payments schema migrated", zap.Uint("version", version))
	return nil
}

// toPgx5URL rewrites a postgres:// or postgresql:// DSN to the pgx5:// scheme
// required by the golang-migrate pgx/v5 database driver.
func toPgx5URL(databaseURL string) string {
	switch {
	case strings.HasPrefix(databaseURL, "postgres://"):
		return "pgx5://" + strings.TrimPrefix(databaseURL, "postgres://")
	case strings.HasPrefix(databaseURL, "postgresql://"):
		return "pgx5://" + strings.TrimPrefix(databaseURL, "postgresql://")
	default:
		return databaseURL
	}
}
