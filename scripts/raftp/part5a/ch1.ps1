
# Set example.com to use aliyun dns
Add-DnsClientNrptRule -Namespace "example.com" -NameServers "114.114.114.114"

# Flush dns cache
ipconfig /flushdns

# Add aliyun dns to use athernet in route
route add 114.114.114.0 mask 255.255.255.0 "<ipv4>" metric 5

# Add example.com to use athernet in route
route add 93.184.216.0 mask 255.255.255.0 "<ipv4>" metric 5
