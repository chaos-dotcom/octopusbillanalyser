import os
import pandas as pd
import pytesseract
from PIL import Image
import re
from datetime import datetime
import hashlib

def extract_date(text):
    """Extract date from bill text using regex patterns."""
    # Common date formats in bills
    date_patterns = [
        r'(\d{1,2}[/-]\d{1,2}[/-]\d{2,4})',  # DD/MM/YYYY or MM/DD/YYYY
        r'(\d{1,2}\s+(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*\s+\d{2,4})',  # DD Month YYYY
        r'((?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*\s+\d{1,2},?\s+\d{2,4})',  # Month DD, YYYY
        r'(?:\(|-)?\s*(\d{1,2}(?:st|nd|rd|th)?\s+(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*\s+\d{4})\s*(?:\)|-)?' # Format like (11th March 2025)
    ]
    
    for pattern in date_patterns:
        matches = re.findall(pattern, text, re.IGNORECASE)
        if matches:
            return matches[0]
    
    return None

def extract_amount(text):
    """Extract monetary amounts from bill text."""
    # Look for currency symbols followed by numbers
    amount_patterns = [
        r'[$£€](\d+\.\d{2})',  # $123.45
        r'£(\d+\.\d{2})',  # £123.45 (specific for pound symbol)
        r'(\d+\.\d{2})[$£€]',  # 123.45$
        r'Total:?\s*[$£€]?(\d+\.\d{2})',  # Total: $123.45
        r'Amount\s*due:?\s*[$£€]?(\d+\.\d{2})',  # Amount due: $123.45
        r'Total\s+(?:Electricity|Gas)?\s+Charges\s*[£$€]?(\d+\.\d{2})',  # Total Electricity Charges £3.08
        r'Total\s+charges\s+for\s+bill\s*[£$€]?(\d+\.\d{2})'  # Total charges for bill £3.08
    ]
    
    for pattern in amount_patterns:
        matches = re.findall(pattern, text, re.IGNORECASE)
        if matches:
            return matches[0]
    
    return None

def extract_bill_type(text):
    """Determine if the bill is for gas or electricity."""
    if re.search(r'gas', text, re.IGNORECASE):
        return 'Gas'
    elif re.search(r'electric|electricity', text, re.IGNORECASE):
        return 'Electric'
    else:
        return 'Unknown'

def extract_account_number(text):
    """Extract account number from bill text."""
    account_patterns = [
        r'Account\s*(?:Number|No|#)?\s*:?\s*(\d+[-\s]?\d+)',
        r'Account\s*(?:Number|No|#)?\s*:?\s*([A-Z0-9]+)',
        r'Supply\s+number\s*:?\s*([A-Z0-9]+)'  # Supply number: 19000232...
    ]
    
    for pattern in account_patterns:
        matches = re.findall(pattern, text, re.IGNORECASE)
        if matches:
            return matches[0]
    
    return None

def extract_tariff_and_billing_period(text):
    """Extract tariff name, start date, and end date from bill text."""
    # Pattern for "Tariff Name (StartDate - EndDate)"
    # Example: "Cosy Octopus (12th May 2025 - 24th May 2025)"
    
    date_pattern_part = r'\d{1,2}(?:st|nd|rd|th)?\s+(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*\s+\d{2,4}'
    
    tariff_names = [
        "Cosy Octopus", "Agile Octopus", "Octopus Tracker",
        # Add more known tariff names if needed
    ]
    
    for tariff_name_base in tariff_names:
        # Regex to capture the tariff name itself and the two dates within parentheses
        pattern_str = rf'({re.escape(tariff_name_base)})\s*\(\s*({date_pattern_part})\s*-\s*({date_pattern_part})\s*\)'
        
        match = re.search(pattern_str, text, re.IGNORECASE)
        if match:
            tariff = match.group(1).strip()  # The matched tariff name
            start_date_str = match.group(2).strip()
            end_date_str = match.group(3).strip()
            return tariff, start_date_str, end_date_str
            
    return None, None, None

def calculate_fingerprint(text):
    """Create a fingerprint of the bill to help identify duplicates."""
    # Remove all whitespace and convert to lowercase
    normalized_text = re.sub(r'\s+', '', text.lower())
    # Create a hash of the text
    return hashlib.md5(normalized_text.encode()).hexdigest()

def extract_meter_number(text):
    """Extract meter number from bill text."""
    meter_patterns = [
        r'Meter\s+(?:Number|No|#)?\s*:?\s*([A-Z0-9]+)',
        r'(?:for|from)\s+Meter\s+([A-Z0-9]+)'  # Energy Charges for Meter 17K0160497
    ]
    
    for pattern in meter_patterns:
        matches = re.findall(pattern, text, re.IGNORECASE)
        if matches:
            return matches[0]
    
    return None

def extract_address(text):
    """Extract address from bill text."""
    address_patterns = [
        r'Supply\s+Address:?\s*(.*?)(?:Postcode|$)',
        r'Address:?\s*(.*?)(?:Postcode|$)'
    ]
    
    for pattern in address_patterns:
        matches = re.search(pattern, text, re.IGNORECASE | re.DOTALL)
        if matches:
            # Clean up the address
            address = matches.group(1).strip()
            # Remove extra whitespace and newlines
            address = re.sub(r'\s+', ' ', address)
            return address
    
    return None

def process_bill_images(folder_path):
    """Process all image files in the given folder."""
    # Supported image extensions
    image_extensions = ['.jpg', '.jpeg', '.png', '.tiff', '.bmp']
    
    bill_data = []
    
    for filename in os.listdir(folder_path):
        file_path = os.path.join(folder_path, filename)
        file_ext = os.path.splitext(filename)[1].lower()
        
        if os.path.isfile(file_path) and file_ext in image_extensions:
            try:
                # Open the image
                image = Image.open(file_path)
                
                # Perform OCR
                text = pytesseract.image_to_string(image)
                
                # Extract information
                date = extract_date(text) # General date, could be issue date
                tariff, start_date, end_date = extract_tariff_and_billing_period(text)
                amount = extract_amount(text)
                bill_type = extract_bill_type(text)
                account_number = extract_account_number(text)
                meter_number = extract_meter_number(text)
                address = extract_address(text)
                fingerprint = calculate_fingerprint(text)
                
                bill_data.append({
                    'Filename': filename,
                    'Date': date, # General bill date
                    'Tariff': tariff,
                    'Start Date': start_date,
                    'End Date': end_date,
                    'Amount': amount,
                    'Type': bill_type,
                    'Account Number': account_number,
                    'Meter Number': meter_number,
                    'Address': address,
                    'Fingerprint': fingerprint
                })
                
                print(f"Processed: {filename}")
            except Exception as e:
                print(f"Error processing {filename}: {e}")
    
    return bill_data

def identify_duplicates(bill_data):
    """Identify potential duplicate bills based on fingerprints and other data."""
    df = pd.DataFrame(bill_data)
    
    # Find exact fingerprint matches
    fingerprint_counts = df['Fingerprint'].value_counts()
    duplicate_fingerprints = fingerprint_counts[fingerprint_counts > 1].index.tolist()
    
    duplicates = []
    
    # Check for exact fingerprint matches
    for fingerprint in duplicate_fingerprints:
        matching_files = df[df['Fingerprint'] == fingerprint]['Filename'].tolist()
        duplicates.append({
            'Files': matching_files,
            'Match Type': 'Exact content match',
            'Fingerprint': fingerprint
        })
    
    # Check for bills with same date, amount and type but different fingerprints
    if 'Date' in df.columns and 'Amount' in df.columns and 'Type' in df.columns:
        # Group by these fields and find groups with more than one bill
        potential_dupes = df.groupby(['Date', 'Amount', 'Type']).filter(lambda x: len(x) > 1)
        
        for _, group in potential_dupes.groupby(['Date', 'Amount', 'Type']):
            if len(group) > 1 and len(set(group['Fingerprint'])) > 1:  # Different fingerprints
                duplicates.append({
                    'Files': group['Filename'].tolist(),
                    'Match Type': 'Same date, amount and type',
                    'Date': group['Date'].iloc[0],
                    'Amount': group['Amount'].iloc[0],
                    'Type': group['Type'].iloc[0]
                })
    
    return duplicates

def main():
    # Get the current directory
    current_dir = os.getcwd()
    
    print("Starting bill analysis...")
    bill_data = process_bill_images(current_dir)
    
    if not bill_data:
        print("No bill images found or processed.")
        return
    
    # Create a DataFrame for all bills
    df = pd.DataFrame(bill_data)

    # Sort by 'Start Date' if the column exists and has sortable data
    if 'Start Date' in df.columns and not df['Start Date'].isnull().all():
        # Create a temporary column for datetime conversion to sort
        # errors='coerce' will turn unparseable dates into NaT (Not a Time)
        # This preserves the original 'Start Date' string column
        df['Start Date DT'] = pd.to_datetime(df['Start Date'], errors='coerce')
        
        # Sort by the new datetime column, most recent first
        # NaT values (unparseable/missing dates) will be placed last
        df = df.sort_values(by='Start Date DT', ascending=False, na_position='last')
        
        # Remove the temporary datetime column as it's no longer needed
        df = df.drop(columns=['Start Date DT'])

    # Calculate total amount before filling NaN for the 'Amount' column specifically
    # Ensure 'Amount' column exists and has some data
    if 'Amount' in df.columns and not df['Amount'].isnull().all():
        # Convert 'Amount' to numeric, errors='coerce' will turn non-numbers into NaN
        numeric_amounts = pd.to_numeric(df['Amount'], errors='coerce')
        total_amount = numeric_amounts.sum()
        
        # Create summary rows
        total_row = {'Filename': 'Total', 'Amount': round(total_amount, 2)}
        split_row = {'Filename': 'Company/Personal Split (50%)', 'Amount': round(total_amount / 2, 2)}
        
        # Append summary rows to the DataFrame
        # Use pd.concat to append dictionaries as new rows
        summary_df = pd.DataFrame([total_row, split_row])
        df = pd.concat([df, summary_df], ignore_index=True)

    # Format the DataFrame for display and CSV output
    # Fill NaN values (which includes original None values for dates that couldn't be parsed by to_datetime,
    # or if 'Start Date' was None initially) with " " for better readability.
    # This will also fill other columns for the summary rows.
    df = df.fillna(" ")
    
    # Save to CSV
    csv_path = os.path.join(current_dir, 'bill_data.csv')
    df.to_csv(csv_path, index=False)
    print(f"Bill data saved to {csv_path}")
    
    # Print a summary of the extracted data
    print("\nExtracted Bill Information:")
    for i, bill in enumerate(bill_data, 1):
        print(f"\nBill {i}: {bill['Filename']}")
        print(f"  Type: {bill.get('Type', ' ')}")
        print(f"  Date: {bill.get('Date', ' ')}") # General bill date
        print(f"  Tariff: {bill.get('Tariff', ' ')}")
        print(f"  Period Start: {bill.get('Start Date', ' ')}")
        print(f"  Period End: {bill.get('End Date', ' ')}")
        print(f"  Amount: £{bill.get('Amount', ' ') if bill.get('Amount') != ' ' else ' '}") # Ensure 'Amount' key exists
        print(f"  Account Number: {bill.get('Account Number', ' ')}")
        print(f"  Meter Number: {bill.get('Meter Number', ' ')}")
        print(f"  Address: {bill.get('Address', ' ')}")
    
    # Find duplicates
    duplicates = identify_duplicates(bill_data)
    
    if duplicates:
        print("\nPotential duplicate bills found:")
        for i, dupe in enumerate(duplicates, 1):
            print(f"\nDuplicate Set {i}:")
            print(f"Match Type: {dupe['Match Type']}")
            print(f"Files: {', '.join(dupe['Files'])}")
            
            if 'Date' in dupe:
                print(f"Date: {dupe['Date']}")
            if 'Amount' in dupe:
                print(f"Amount: {dupe['Amount']}")
            if 'Type' in dupe:
                print(f"Type: {dupe['Type']}")
        
        # Save duplicates to CSV
        dupes_df = pd.DataFrame(duplicates)
        dupes_csv_path = os.path.join(current_dir, 'duplicate_bills.csv')
        dupes_df.to_csv(dupes_csv_path, index=False)
        print(f"\nDuplicate information saved to {dupes_csv_path}")
    else:
        print("\nNo duplicate bills found.")

if __name__ == "__main__":
    main()
