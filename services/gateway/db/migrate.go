// Package db provides embedded SQL migrations for the Blazil gateway service.
package db

import (
	"embed"
	"errors"
	"fmt"
	"strings"

	"github.com/golang-migrate/migrate/v4"
	_ "github.com/golang-migrate/migrate/v4/database/pgx/v5"
	"github.com/golang-migrate/migrate/v4/source/iofs"
	"go.uber.org/zap"
)

//go:embed migrations/*.sql
var migrationsFS embed.FS

// RunMigrations applies all pending UP migrations embedded in this package.
// It is safe to call on every service startup — already-applied migrations
// are skipped, and a clean database receives all migrations in order.
//
// databaseURL must use the postgres:// or postgresql:// scheme.
func RunMigrations(databaseURL string, logger *zap.Logger) error {
	src, err := iofs.New(migrationsFS, "migrations")
	if err != nil {
		return fmt.Errorf("db: iofs source: %w", err)
	}

	m, err := migrate.NewWithSourceInstance("iofs", src, toPgx5URL(databaseURL))
	if err != nil {
		return fmt.Errorf("db: migrate init: %w", err)
	}
	defer m.Close()

	ver, dirty, err := m.Version()
	if err != nil && !errors.Is(err, migrate.ErrNilVersion) {
		return fmt.Errorf("db: version check: %w", err)
	}
	if dirty {
		return fmt.Errorf("db: schema is dirty at version %d — manual rollback required", ver)
	}
	logger.Info("db: current schema version", zap.Uint("version", ver))

	if err := m.Up(); err != nil && !errors.Is(err, migrate.ErrNoChange) {
		return fmt.Errorf("db: migrate up: %w", err)
	}

	ver, _, _ = m.Version()
	logger.Info("db: schema up to date", zap.Uint("version", ver))
	return nil
}

// toPgx5URL converts postgres:// / postgresql:// to pgx5:// as required by
// the golang-migrate pgx/v5 database driver.
func toPgx5URL(u string) string {
	switch {
	case strings.HasPrefix(u, "postgres://"):
		return "pgx5://" + u[len("postgres://"):]
	case strings.HasPrefix(u, "postgresql://"):
		return "pgx5://" + u[len("postgresql://"):]
	default:
		return u
	}
}
