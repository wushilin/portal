acl {
  priority = 100
  action = "allow"

  from = [
    "10.0.0.0/16",
    "192.168.1.0/24",
  ]

  ignore_ip = true
  to {
    hosts = [
      "^api\\.example\\.com$",
      ".*\\.internal\\.example\\.com$",
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
  action = "allow"
  from   = ["0.0.0.0/0"]

  to {
    hosts = ["somehost"]
    ports = [22]
  }
}

acl {
  priority = 0
  action = "allow"
  from = ["::1"]
  to {
    hosts = ["::1"]
    ports = ["22"]
  }
}