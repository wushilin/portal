acl {
  priority = 100
  action = "allow"
  from = [
    "10.0.0.0/16",
    "192.168.1.0/24",
  ]
  to {
    hosts = [
      "^api\.example\.com$",
      ".*\.internal\.example\.com$",
    ]
    ports = [
      80,
      443,
      "8000-8100"
    ]
  }
}

acl {
  priority = 50
  action = "deny"
  from = [
    "0.0.0.0/0",
  ]
  to {
    hosts = [
      ".*",
    ]
    ports = [
      22,
    ]
  }
}

