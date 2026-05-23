// export_test.go exposes internal identifiers for use by the proxy_test package.
// This file is only compiled during `go test` — never included in production builds.
package proxy

// NewDirectorForTest creates a Director with the given routes (test helper).
func NewDirectorForTest(routes []Route) *Director {
	return NewDirector(routes)
}

// RouteForTest exposes the unexported routeFor method so proxy_test can
// verify routing logic without a real gRPC connection.
func (d *Director) RouteForTest(fullMethod string) string {
	return d.routeFor(fullMethod)
}
