use std::fs;

pub mod aclutil;
pub mod util;


#[tokio::main]
async fn main() {
    let acl_file = aclutil::load_acl("acl.hcl").expect("Failed to load ACL file");
    println!("Loaded ACL file: {:?}", acl_file);
    
    // Serialize to HCL format (using custom formatter to match original block format)
    let hcl_string = aclutil::serialize_acl_to_hcl(&acl_file);
    
    // Write to acl2.hcl
    fs::write("acl2.hcl", hcl_string).expect("Failed to write acl2.hcl");
    
    println!("Serialized ACL file written to acl2.hcl");
    
    // Test evaluate function
    println!("\nTesting evaluate function:");
    
    // Test case 1: Should match first rule (allow) - IP in 10.0.0.0/16, host matches pattern, port 80
    let result1 = acl_file.evaluate("10.0.0.1", "api.example.com:80");
    println!("10.0.0.1 -> api.example.com:80: {:?}", result1);
    
    // Test case 2: Should match second rule (deny) - IP in 0.0.0.0/0, host matches .*, port 22
    let result2 = acl_file.evaluate("192.168.1.1", "anyhost.com:22");
    println!("192.168.1.1 -> anyhost.com:22: {:?}", result2);
    
    // Test case 3: Should not match any rule, default deny
    let result3 = acl_file.evaluate("8.8.8.8", "example.com:9999");
    println!("8.8.8.8 -> example.com:9999: {:?}", result3);
    
    // Test case 4: Should match first rule - port range
    let result4 = acl_file.evaluate("10.0.0.5", "api.example.com:8050");
    println!("10.0.0.5 -> api.example.com:8050: {:?}", result4);
}