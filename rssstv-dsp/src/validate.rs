use crate::DspError;

pub(crate) fn validate_order(order: usize) -> Result<(), DspError> {
    if order == 0 || !order.is_multiple_of(2) {
        Err(DspError::InvalidOrder)
    } else {
        Ok(())
    }
}

pub(crate) fn validate_frequency(sample_rate_hz: f64, frequency_hz: f64) -> Result<(), DspError> {
    if !sample_rate_hz.is_finite() || sample_rate_hz <= 0.0 {
        return Err(DspError::InvalidSampleRate);
    }
    if !frequency_hz.is_finite() || frequency_hz <= 0.0 || frequency_hz >= sample_rate_hz * 0.5 {
        return Err(DspError::InvalidFrequency);
    }
    Ok(())
}

pub(crate) fn validate_frequency_range(
    sample_rate_hz: f64,
    lower_frequency_hz: f64,
    upper_frequency_hz: f64,
) -> Result<(), DspError> {
    validate_frequency(sample_rate_hz, lower_frequency_hz)?;
    validate_frequency(sample_rate_hz, upper_frequency_hz)?;
    if lower_frequency_hz >= upper_frequency_hz {
        return Err(DspError::InvalidFrequency);
    }
    Ok(())
}
