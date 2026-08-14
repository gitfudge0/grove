use gpui::Window;

/// Semantic content type for an [`Input`](super::Input); variants mirror Swift's text content types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputContentType {
    Name,
    NamePrefix,
    GivenName,
    MiddleName,
    FamilyName,
    NameSuffix,
    Nickname,
    JobTitle,
    OrganizationName,
    Location,
    FullStreetAddress,
    StreetAddressLine1,
    StreetAddressLine2,
    AddressCity,
    AddressState,
    AddressCityAndState,
    Sublocality,
    CountryName,
    PostalCode,
    TelephoneNumber,
    EmailAddress,
    Url,
    CreditCardNumber,
    CreditCardName,
    CreditCardGivenName,
    CreditCardMiddleName,
    CreditCardFamilyName,
    CreditCardSecurityCode,
    CreditCardExpiration,
    CreditCardExpirationMonth,
    CreditCardExpirationYear,
    CreditCardType,
    Username,
    Password,
    NewPassword,
    OneTimeCode,
    ShipmentTrackingNumber,
    FlightNumber,
    DateTime,
    Birthdate,
    BirthdateDay,
    BirthdateMonth,
    BirthdateYear,
    CellularEid,
    CellularImei,
}

impl InputContentType {
    // GROVE LOCAL PATCH (re-apply on re-vendor): gate matches the sole caller so this isn't never-used under clippy -D warnings on macOS.
    #[cfg(all(target_os = "macos", not(feature = "grove-test-support")))]
    pub(crate) const fn ns_text_content_type(self) -> Option<&'static str> {
        match self {
            Self::Name => Some("name"),
            Self::NamePrefix => Some("honorific-prefix"),
            Self::GivenName => Some("given-name"),
            Self::MiddleName => Some("additional-name"),
            Self::FamilyName => Some("family-name"),
            Self::NameSuffix => Some("honorific-suffix"),
            Self::Nickname => Some("nickname"),
            Self::JobTitle => Some("organization-title"),
            Self::OrganizationName => Some("organization"),
            Self::Location => Some("location"),
            Self::FullStreetAddress => Some("street-address"),
            Self::StreetAddressLine1 => Some("address-line1"),
            Self::StreetAddressLine2 => Some("address-line2"),
            Self::AddressCity => Some("address-level2"),
            Self::AddressState => Some("address-level1"),
            Self::AddressCityAndState => Some("address-level1+2"),
            Self::Sublocality => Some("address-level3"),
            Self::CountryName => Some("country-name"),
            Self::PostalCode => Some("postal-code"),
            Self::TelephoneNumber => Some("tel"),
            Self::EmailAddress => Some("email"),
            Self::Url => Some("url"),
            Self::CreditCardNumber => Some("cc-number"),
            Self::CreditCardName => Some("cc-name"),
            Self::CreditCardGivenName => Some("cc-given-name"),
            Self::CreditCardMiddleName => Some("cc-additional-name"),
            Self::CreditCardFamilyName => Some("cc-family-name"),
            Self::CreditCardSecurityCode => Some("cc-csc"),
            Self::CreditCardExpiration => Some("cc-exp"),
            Self::CreditCardExpirationMonth => Some("cc-exp-month"),
            Self::CreditCardExpirationYear => Some("cc-exp-year"),
            Self::CreditCardType => Some("cc-type"),
            Self::Username => Some("username"),
            Self::Password => Some("password"),
            Self::NewPassword => Some("new-password"),
            Self::OneTimeCode => Some("one-time-code"),
            Self::ShipmentTrackingNumber => Some("shipment-tracking-number"),
            Self::FlightNumber => Some("flight-number"),
            Self::DateTime => Some("date-time"),
            Self::Birthdate => Some("bday"),
            Self::BirthdateDay => Some("bday-day"),
            Self::BirthdateMonth => Some("bday-month"),
            Self::BirthdateYear => Some("bday-year"),
            Self::CellularEid | Self::CellularImei => None,
        }
    }
}

pub(super) fn sync_native_content_type(
    window: &mut Window,
    content_type: Option<InputContentType>,
    disabled: bool,
) {
    if disabled {
        return;
    }

    // `grove-test-support` skips native sync in tests: gpui's test window panics `unimplemented!()` on `window_handle`, which `native::ns_view` can't absorb.
    #[cfg(all(target_os = "macos", not(feature = "grove-test-support")))]
    super::native::set_text_content_type(window, content_type);

    #[cfg(any(not(target_os = "macos"), feature = "grove-test-support"))]
    let _ = (window, content_type);
}

// GROVE LOCAL PATCH (re-apply on re-vendor): same gate as `ns_text_content_type` above.
#[cfg(all(test, target_os = "macos", not(feature = "grove-test-support")))]
mod tests {
    use super::*;

    #[test]
    fn content_type_maps_to_ns_text_content_type_values() {
        let content_types = [
            (InputContentType::Name, Some("name")),
            (InputContentType::NamePrefix, Some("honorific-prefix")),
            (InputContentType::GivenName, Some("given-name")),
            (InputContentType::MiddleName, Some("additional-name")),
            (InputContentType::FamilyName, Some("family-name")),
            (InputContentType::NameSuffix, Some("honorific-suffix")),
            (InputContentType::Nickname, Some("nickname")),
            (InputContentType::JobTitle, Some("organization-title")),
            (InputContentType::OrganizationName, Some("organization")),
            (InputContentType::Location, Some("location")),
            (InputContentType::FullStreetAddress, Some("street-address")),
            (InputContentType::StreetAddressLine1, Some("address-line1")),
            (InputContentType::StreetAddressLine2, Some("address-line2")),
            (InputContentType::AddressCity, Some("address-level2")),
            (InputContentType::AddressState, Some("address-level1")),
            (
                InputContentType::AddressCityAndState,
                Some("address-level1+2"),
            ),
            (InputContentType::Sublocality, Some("address-level3")),
            (InputContentType::CountryName, Some("country-name")),
            (InputContentType::PostalCode, Some("postal-code")),
            (InputContentType::TelephoneNumber, Some("tel")),
            (InputContentType::EmailAddress, Some("email")),
            (InputContentType::Url, Some("url")),
            (InputContentType::CreditCardNumber, Some("cc-number")),
            (InputContentType::CreditCardName, Some("cc-name")),
            (InputContentType::CreditCardGivenName, Some("cc-given-name")),
            (
                InputContentType::CreditCardMiddleName,
                Some("cc-additional-name"),
            ),
            (
                InputContentType::CreditCardFamilyName,
                Some("cc-family-name"),
            ),
            (InputContentType::CreditCardSecurityCode, Some("cc-csc")),
            (InputContentType::CreditCardExpiration, Some("cc-exp")),
            (
                InputContentType::CreditCardExpirationMonth,
                Some("cc-exp-month"),
            ),
            (
                InputContentType::CreditCardExpirationYear,
                Some("cc-exp-year"),
            ),
            (InputContentType::CreditCardType, Some("cc-type")),
            (InputContentType::Username, Some("username")),
            (InputContentType::Password, Some("password")),
            (InputContentType::NewPassword, Some("new-password")),
            (InputContentType::OneTimeCode, Some("one-time-code")),
            (
                InputContentType::ShipmentTrackingNumber,
                Some("shipment-tracking-number"),
            ),
            (InputContentType::FlightNumber, Some("flight-number")),
            (InputContentType::DateTime, Some("date-time")),
            (InputContentType::Birthdate, Some("bday")),
            (InputContentType::BirthdateDay, Some("bday-day")),
            (InputContentType::BirthdateMonth, Some("bday-month")),
            (InputContentType::BirthdateYear, Some("bday-year")),
            (InputContentType::CellularEid, None),
            (InputContentType::CellularImei, None),
        ];

        for (content_type, native_value) in content_types {
            assert_eq!(content_type.ns_text_content_type(), native_value);
        }
    }
}
