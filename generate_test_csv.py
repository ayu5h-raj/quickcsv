#!/usr/bin/env python3
"""Generate a 100MB CSV file for testing FastCSV."""

import csv
import random
import string
import os

OUTPUT_FILE = "large_test.csv"
TARGET_SIZE_MB = 2000
TARGET_SIZE_BYTES = TARGET_SIZE_MB * 1024 * 1024

# Column definitions
DEPARTMENTS = ["Engineering", "Marketing", "Sales", "HR", "Finance", "Operations", "Legal", "Support"]
CITIES = ["New York", "San Francisco", "Los Angeles", "Chicago", "Seattle", "Austin", "Boston", "Denver"]

def random_string(length):
    return ''.join(random.choices(string.ascii_letters, k=length))

def random_email(name):
    domains = ["example.com", "company.org", "corp.net", "business.io"]
    return f"{name.lower().replace(' ', '.')}@{random.choice(domains)}"

def generate_row(row_id):
    first_name = random_string(random.randint(4, 10)).capitalize()
    last_name = random_string(random.randint(5, 12)).capitalize()
    name = f"{first_name} {last_name}"
    email = random_email(name)
    department = random.choice(DEPARTMENTS)
    city = random.choice(CITIES)
    salary = random.randint(45000, 250000)
    years_exp = random.randint(0, 35)
    start_date = f"20{random.randint(10, 24):02d}-{random.randint(1, 12):02d}-{random.randint(1, 28):02d}"
    description = random_string(random.randint(20, 80))
    
    return [row_id, name, email, department, city, salary, years_exp, start_date, description]

def main():
    headers = ["id", "name", "email", "department", "city", "salary", "years_experience", "start_date", "description"]
    
    print(f"Generating {TARGET_SIZE_MB}MB CSV file...")
    
    with open(OUTPUT_FILE, 'w', newline='') as f:
        writer = csv.writer(f)
        writer.writerow(headers)
        
        row_id = 1
        while True:
            # Write rows in batches of 10000
            for _ in range(10000):
                writer.writerow(generate_row(row_id))
                row_id += 1
            
            # Check file size
            current_size = f.tell()
            progress = (current_size / TARGET_SIZE_BYTES) * 100
            print(f"\rProgress: {progress:.1f}% ({current_size / (1024*1024):.1f}MB) - {row_id:,} rows", end="")
            
            if current_size >= TARGET_SIZE_BYTES:
                break
    
    final_size = os.path.getsize(OUTPUT_FILE)
    print(f"\n\nDone! Created '{OUTPUT_FILE}'")
    print(f"  Size: {final_size / (1024*1024):.2f} MB")
    print(f"  Rows: {row_id:,}")

if __name__ == "__main__":
    main()
