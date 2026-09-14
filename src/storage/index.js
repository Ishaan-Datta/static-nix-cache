'use strict';

const LocalStorage = require('./local');
const S3Storage = require('./s3');
const GitHubReleasesStorage = require('./github-releases');

/**
 * Create the configured storage backend.
 * @param {object} config - application config
 * @returns {LocalStorage|S3Storage|GitHubReleasesStorage}
 */
function createStorage(config) {
  if (config.storageBackend === 's3') {
    return new S3Storage(config.s3);
  }
  if (config.storageBackend === 'github-releases') {
    return new GitHubReleasesStorage({
      ...config.github,
      localPath: config.localStoragePath,
    });
  }
  return new LocalStorage(config.localStoragePath);
}

module.exports = { createStorage };
