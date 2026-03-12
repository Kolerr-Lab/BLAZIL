package blazil

default allow = false

# Payments: subject must carry payment:write role and amount must be positive
allow {
    input.action == "payment:write"
    "payment:write" in input.roles
    input.amount > 0
}

# Trading: subject must carry trading:write role
allow {
    input.action == "order:place"
    "trading:write" in input.roles
}

# Balance read: subject may only read their own balance
allow {
    input.action == "balance:read"
    input.subject == input.resource_owner
}

# Admins may do anything
allow {
    "admin" in input.roles
}
